use {
    thiserror::Error,
    crate::function::abi::{function_memory::{FunctionMemoryError, FunctionMemoryAccessError, FunctionMemoryGetStringError}},
};

#[derive(Debug, Error)]
pub(crate) enum BlobPutError {
    #[error("error in storage implementation")]
    StorageError,
}

impl From<crate::tasks::blob::PutError> for BlobPutError {
    fn from(err: crate::tasks::blob::PutError) -> Self {
        use crate::tasks::blob::PutError as SourceError;
        match err {
            SourceError::BlobStorageError => Self::StorageError,
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum BlobGetError {
    #[error("bad request: failed to access memory")]
    BadRequestFailedToAccessMemory,
    #[error("bad request: argument out of bounds")]
    BadRequestArgumentOutOfBounds,
    #[error("bad request: argument failed to decode")]
    BadRequestArgumentFailedToDecode,

    #[error("binding does not exist")]
    BindingNotExists,

    #[error("error in storage implementation")]
    StorageError,
}

impl From<FunctionMemoryError> for Result<Option<Vec<u8>>, BlobGetError> {
    fn from(value: FunctionMemoryError) -> Self {
        match value {
            FunctionMemoryError::MemoryNotFound | FunctionMemoryError::MemoryNotMemory => Err(BlobGetError::BadRequestFailedToAccessMemory),
        }
    }
}

impl From<FunctionMemoryAccessError> for Result<Option<Vec<u8>>, BlobGetError> {
    fn from(value: FunctionMemoryAccessError) -> Self {
        match value {
            FunctionMemoryAccessError::OutOfBounds => Err(BlobGetError::BadRequestArgumentOutOfBounds),
        }
    }
}

impl From<FunctionMemoryGetStringError> for Result<Option<Vec<u8>>, BlobGetError> {
    fn from(value: FunctionMemoryGetStringError) -> Self {
        match value {
            FunctionMemoryGetStringError::OutOfBounds => Err(BlobGetError::BadRequestArgumentOutOfBounds),
            FunctionMemoryGetStringError::FailedToDecode => Err(BlobGetError::BadRequestArgumentFailedToDecode),
        }
    }
}

#[derive(Debug, Error)]
pub(crate) enum BlobDeleteError {
    #[error("failed to delete object because of unexpected error in blob storage implementation")]
    StorageError,
}

impl From<crate::tasks::blob::DeleteError> for BlobDeleteError {
    fn from(err: crate::tasks::blob::DeleteError) -> Self {
        use crate::tasks::blob::DeleteError as SourceError;
        match err {
            SourceError::BlobStorageError => Self::StorageError,
        }
    }
}
