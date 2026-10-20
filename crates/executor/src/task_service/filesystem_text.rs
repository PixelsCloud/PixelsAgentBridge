// Shared with Android; storage and publication stay in this executor.
pub(super) use pab_file_codec::{decode, encode, newline, read};

#[cfg(test)]
pub(super) use pab_file_codec::patch;
