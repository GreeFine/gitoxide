#![forbid(unsafe_code)]
#![deny(rust_2018_idioms)]

use bstr::{BStr, BString, ByteSlice};

/// For convenience to allow using `bstr` without adding it to own cargo manifest
pub use bstr;

/// Objects sharing data with a backing store to minimize allocations
pub mod borrowed;
/// Mutable objects with each field being separately allocated and mutable.
pub mod owned;

mod types;
pub use types::*;

pub mod commit;

/// Denotes the kind of hash used to identify objects.
#[derive(PartialEq, Eq, Debug, Hash, Ord, PartialOrd, Clone, Copy)]
#[cfg_attr(feature = "serde1", derive(serde::Serialize, serde::Deserialize))]
pub enum HashKind {
    /// The Sha1 hash with 160 bits.
    Sha1,
}

impl Default for HashKind {
    fn default() -> Self {
        HashKind::Sha1
    }
}
