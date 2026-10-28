//! Native file management shared by the local browser and remote adapters.
#![forbid(unsafe_code)]

mod path;
mod native_paths;
pub use native_paths::{keep_both_candidate,native_join, native_name, native_parent};
mod mutation;
pub use mutation::{create_directory, create_directory_native, decode_component, delete, rename, rename_native, validate_new_name};
mod inspect;
mod path_identity;
pub use path_identity::{filesystem_scope,inspect_path_identity};
mod file_attributes;
mod directory_attributes;
#[cfg(unix)]
mod directory_staging;
#[cfg(unix)]
pub use directory_staging::{recover_directory_staging,DirectoryStagingRecovery,directory_creation_receipts,DirectoryCreationReceipt,observe_directory_creation,DirectoryCreationObservation};
pub use directory_attributes::DirectoryObject;
pub use file_attributes::{apply_file_metadata,observe_file_metadata,file_transfer_metadata,modified_at_unix_ms,verify_source_metadata,verify_opened_source_metadata};
mod locations;
pub use locations::locations;
pub use inspect::inspect;
pub use path::{decode_path, display_name, encode_path};
mod directory;
pub use directory::DirectoryIndex;
mod local_manager;
pub use local_manager::{LocalFileManager,LocalFileBrowser,FileMutationReceipt};
mod batch;
mod sqlite;
#[doc(hidden)]
pub use sqlite::begin_write_transaction;
mod scan_store;
mod attempt_store;
pub use attempt_store::AttemptEvidence;
mod target_store;
mod directory_store;
mod directory_preparation;
pub use directory_preparation::DirectoryPreparation;
pub use directory_store::{DirectoryReady,DirectoryFinalization};
mod runner_lease;
mod elapsed_store;
pub use target_store::ItemTarget;
pub use scan_store::{PlannedItem,ScanDirectory};
mod planner;
pub use planner::{FileBrowser,plan_tree,portable_name};
pub mod publish;
mod anchored_transfer;
mod staging_receipt;
pub use staging_receipt::PrivateFileStagingReceipt;
#[cfg(unix)]
mod private_staging;
#[cfg(unix)]
pub use private_staging::PrivateFileStaging;
#[cfg(any(windows, target_os="linux", target_os="android"))]
mod target_reservation;
#[cfg(all(test, any(windows, target_os="linux")))]
mod target_reservation_tests;
pub use anchored_transfer::{StagingTarget,open_source,open_source_in_parent};
pub use batch::{BatchSnapshot, BatchStore};
#[cfg(test)]
mod batch_tests;
#[cfg(test)]
mod planner_tests;

#[derive(Debug, thiserror::Error)]
pub enum FileManagerError {
    #[error("invalid file-manager request: {0}")]
    Invalid(&'static str),
    #[error("directory snapshot is absent, expired, or belongs to another owner")]
    SnapshotUnavailable,
    #[error("file-manager resource limit reached")]
    Capacity,
    #[error("file-manager operation interrupted")]
    Interrupted,
    #[error("file-manager request conflicts with an existing task")]
    Conflict,
    #[error("file-manager task is absent or belongs to another owner")]
    NotFound,
    #[error("directory mutation has an unconfirmed outcome: {0}")]
    OutcomeUnconfirmed(String),
    #[error("file-manager I/O failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("file-manager storage failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("file-manager stored data is invalid: {0}")]
    Encoding(#[from] serde_json::Error),
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(i64::MAX as u128) as i64
}
