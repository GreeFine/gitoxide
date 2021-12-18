use std::{ops::Deref, option::Option::None, sync::Arc, vec::IntoIter};

use git_hash::ObjectId;

use crate::{general::handle, loose, store::general};

pub enum State {
    Pack {
        index_iter: IntoIter<handle::IndexLookup>,
        index: handle::IndexLookup,
        entry_index: u32,
        num_objects: u32,
    },
    Loose {
        iter: loose::Iter,
        index: usize,
    },
    Depleted,
}

/// An iterator over all objects of a linked database
pub struct AllObjects {
    state: State,
    num_objects: usize,
    loose_dbs: Arc<Vec<loose::Store>>,
}

impl AllObjects {
    /// Create a new iterator from a general database, which will be forced to load all indices eagerly.
    pub fn new(db: &general::Store) -> Result<Self, crate::general::load_index::Error> {
        let mut snapshot = db.collect_snapshot();
        while let Some(new_snapshot) = db.load_one_index(crate::RefreshMode::Never, snapshot.marker)? {
            snapshot = new_snapshot
        }

        let packed_objects = snapshot
            .indices
            .iter()
            .fold(0usize, |dbc, index| dbc.saturating_add(index.num_objects() as usize));
        let mut index_iter = snapshot.indices.into_iter();
        let loose_dbs = snapshot.loose_dbs;
        let state = match index_iter.next() {
            Some(index) => {
                let num_objects = index.num_objects();
                State::Pack {
                    index_iter,
                    index,
                    entry_index: 0,
                    num_objects,
                }
            }
            None => {
                let index = 0;
                State::Loose {
                    iter: loose_dbs.get(index).expect("at least one loose db").iter(),
                    index,
                }
            }
        };
        Ok(AllObjects {
            state,
            loose_dbs,
            num_objects: packed_objects,
        })
    }
}

impl Iterator for AllObjects {
    type Item = Result<ObjectId, loose::iter::Error>;

    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.state {
            State::Depleted => None,
            State::Pack {
                index_iter,
                index,
                entry_index,
                num_objects,
            } => {
                if *entry_index < *num_objects {
                    let oid = index.oid_at_index(*entry_index).to_owned();
                    *entry_index += 1;
                    Some(Ok(oid))
                } else {
                    match index_iter.next() {
                        Some(new_index) => {
                            *index = new_index;
                            *entry_index = 0;
                            *num_objects = index.num_objects();
                        }
                        None => {
                            let index = 0;
                            self.state = State::Loose {
                                iter: self.loose_dbs.get(index).expect("at least one loose odb").iter(),
                                index,
                            }
                        }
                    }
                    self.next()
                }
            }
            State::Loose { iter, index } => match iter.next() {
                Some(id) => Some(id),
                None => {
                    *index += 1;
                    match self.loose_dbs.get(*index).map(|ldb| ldb.iter()) {
                        Some(new_iter) => {
                            *iter = new_iter;
                            self.next()
                        }
                        None => {
                            self.state = State::Depleted;
                            None
                        }
                    }
                }
            },
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.num_objects, None)
    }
}

impl<S> super::Handle<S>
where
    S: Deref<Target = super::Store> + Clone,
{
    /// Return an iterator over all objects in all linked databases, database after database, first packed
    /// objects with the 'best' packs first, followed by loose objects.
    /// For specialized iterations, use the `dbs` fields directly as all databases are accessible.
    pub fn iter(&self) -> Result<AllObjects, general::load_index::Error> {
        AllObjects::new(self.store())
    }
}

impl general::Store {
    /// Like [`Handle::iter()`][super::Handle::iter()], but accessible directly on the store.
    pub fn iter(&self) -> Result<AllObjects, general::load_index::Error> {
        AllObjects::new(self)
    }
}
