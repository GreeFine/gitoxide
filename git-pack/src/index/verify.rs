use std::sync::{atomic::AtomicBool, Arc};

use git_features::progress::{self, Progress};
use git_object::{bstr::ByteSlice, WriteTo};

use crate::index;

///
pub mod integrity {
    use git_object::bstr::BString;

    /// Returned by [`index::File::verify_integrity()`][crate::index::File::verify_integrity()].
    #[derive(thiserror::Error, Debug)]
    #[allow(missing_docs)]
    pub enum Error {
        #[error("{kind} object {id} could not be decoded")]
        ObjectDecode {
            source: git_object::decode::Error,
            kind: git_object::Kind,
            id: git_hash::ObjectId,
        },
        #[error("{kind} object {id} wasn't re-encoded without change, wanted\n{expected}\n\nGOT\n\n{actual}")]
        ObjectEncodeMismatch {
            kind: git_object::Kind,
            id: git_hash::ObjectId,
            expected: BString,
            actual: BString,
        },
    }
}

///
pub mod checksum {
    /// Returned by [`index::File::verify_checksum()`][crate::index::File::verify_checksum()].
    pub type Error = crate::verify::checksum::Error;
}

/// Various ways in which a pack and index can be verified
#[derive(Debug, Eq, PartialEq, Hash, Clone, Copy)]
pub enum Mode {
    /// Validate the object hash and CRC32
    HashCrc32,
    /// Validate hash and CRC32, and decode each non-Blob object.
    /// Each object should be valid, i.e. be decodable.
    HashCrc32Decode,
    /// Validate hash and CRC32, and decode and encode each non-Blob object.
    /// Each object should yield exactly the same hash when re-encoded.
    HashCrc32DecodeEncode,
}

/// Information to allow verifying the integrity of an index with the help of its corresponding pack.
pub struct PackContext<'a, C, F>
where
    C: crate::cache::DecodeEntry,
    F: Fn() -> C + Send + Clone,
{
    /// The pack data file itself.
    pub data: &'a crate::data::File,
    /// the way to verify the pack data.
    pub verify_mode: Mode,
    /// The traversal algorithm to use
    pub traversal_algorithm: index::traverse::Algorithm,
    /// A function to create a pack cache for each tread.
    pub make_cache_fn: F,
}

/// Verify and validate the content of the index file
impl index::File {
    /// Returns the trailing hash stored at the end of this index file.
    ///
    /// It's a hash over all bytes of the index.
    pub fn index_checksum(&self) -> git_hash::ObjectId {
        git_hash::ObjectId::from(&self.data[self.data.len() - self.hash_len..])
    }

    /// Returns the hash of the pack data file that this index file corresponds to.
    ///
    /// It should [`crate::data::File::checksum()`] of the corresponding pack data file.
    pub fn pack_checksum(&self) -> git_hash::ObjectId {
        let from = self.data.len() - self.hash_len * 2;
        git_hash::ObjectId::from(&self.data[from..][..self.hash_len])
    }

    /// Validate that our [`index_checksum()`][index::File::index_checksum()] matches the actual contents
    /// of this index file, and return it if it does.
    pub fn verify_checksum(
        &self,
        progress: impl Progress,
        should_interrupt: &AtomicBool,
    ) -> Result<git_hash::ObjectId, checksum::Error> {
        crate::verify::checksum_on_disk_or_mmap(
            self.path(),
            &self.data,
            self.index_checksum(),
            self.object_hash,
            progress,
            should_interrupt,
        )
    }

    /// The most thorough validation of integrity of both index file and the corresponding pack data file, if provided.
    /// Returns the checksum of the index file, the traversal outcome and the given progress if the integrity check is successful.
    ///
    /// If `pack` is provided, it is expected (and validated to be) the pack belonging to this index.
    /// It will be used to validate internal integrity of the pack before checking each objects integrity
    /// is indeed as advertised via its SHA1 as stored in this index, as well as the CRC32 hash.
    /// The last member of the Option is a function returning an implementation of [`crate::cache::DecodeEntry`] to be used if
    /// the [`index::traverse::Algorithm`] is `Lookup`.
    /// To set this to `None`, use `None::<(_, _, _, fn() -> crate::cache::Never)>`.
    ///
    /// The `thread_limit` optionally specifies the amount of threads to be used for the [pack traversal][index::File::traverse()].
    /// `make_cache` is only used in case a `pack` is specified, use existing implementations in the [`crate::cache`] module.
    ///
    /// # Tradeoffs
    ///
    /// The given `progress` is inevitably consumed if there is an error, which is a tradeoff chosen to easily allow using `?` in the
    /// error case.
    pub fn verify_integrity<P, C, F>(
        &self,
        pack: Option<PackContext<'_, C, F>>,
        thread_limit: Option<usize>,
        progress: Option<P>,
        should_interrupt: Arc<AtomicBool>,
    ) -> Result<
        (git_hash::ObjectId, Option<index::traverse::Outcome>, Option<P>),
        index::traverse::Error<crate::index::verify::integrity::Error>,
    >
    where
        P: Progress,
        C: crate::cache::DecodeEntry,
        F: Fn() -> C + Send + Clone,
    {
        let mut root = progress::DoOrDiscard::from(progress);
        match pack {
            Some(PackContext {
                data: pack,
                verify_mode: mode,
                traversal_algorithm: algorithm,
                make_cache_fn: make_cache,
            }) => self
                .traverse(
                    pack,
                    root.into_inner(),
                    || {
                        let mut encode_buf = Vec::with_capacity(2048);
                        move |kind, data, index_entry, progress| {
                            Self::verify_entry(mode, &mut encode_buf, kind, data, index_entry, progress)
                        }
                    },
                    make_cache,
                    index::traverse::Options {
                        algorithm,
                        thread_limit,
                        check: index::traverse::SafetyCheck::All,
                        should_interrupt,
                    },
                )
                .map(|(id, outcome, root)| (id, Some(outcome), root)),
            None => self
                .verify_checksum(root.add_child("Sha1 of index"), &should_interrupt)
                .map_err(Into::into)
                .map(|id| (id, None, root.into_inner())),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_entry<P>(
        mode: Mode,
        encode_buf: &mut Vec<u8>,
        object_kind: git_object::Kind,
        buf: &[u8],
        index_entry: &index::Entry,
        progress: &mut P,
    ) -> Result<(), integrity::Error>
    where
        P: Progress,
    {
        if let Mode::HashCrc32Decode | Mode::HashCrc32DecodeEncode = mode {
            use git_object::Kind::*;
            match object_kind {
                Tree | Commit | Tag => {
                    let object = git_object::ObjectRef::from_bytes(object_kind, buf).map_err(|err| {
                        integrity::Error::ObjectDecode {
                            source: err,
                            kind: object_kind,
                            id: index_entry.oid,
                        }
                    })?;
                    if let Mode::HashCrc32DecodeEncode = mode {
                        encode_buf.clear();
                        object
                            .write_to(&mut *encode_buf)
                            .expect("writing to a memory buffer never fails");
                        if encode_buf.as_slice() != buf {
                            let mut should_return_error = true;
                            if let git_object::Kind::Tree = object_kind {
                                if buf.as_bstr().find(b"100664").is_some() || buf.as_bstr().find(b"100640").is_some() {
                                    progress.info(format!("Tree object {} would be cleaned up during re-serialization, replacing mode '100664|100640' with '100644'", index_entry.oid));
                                    should_return_error = false
                                }
                            }
                            if should_return_error {
                                return Err(integrity::Error::ObjectEncodeMismatch {
                                    kind: object_kind,
                                    id: index_entry.oid,
                                    expected: buf.into(),
                                    actual: encode_buf.clone().into(),
                                });
                            }
                        }
                    }
                }
                Blob => {}
            };
        }
        Ok(())
    }
}
