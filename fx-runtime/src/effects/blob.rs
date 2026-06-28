use crate::function::abi::{function_memory::{FunctionMemoryError, FunctionMemoryAccessError, FunctionMemoryGetStringError}};

pub(crate) enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BadRequestFailedToAccessMemory,
    BadRequestArgumentOutOfBounds,
    BadRequestArgumentFailedToDecode,
    BindingNotExists,
}

impl From<FunctionMemoryError> for BlobGetResponse {
    fn from(value: FunctionMemoryError) -> Self {
        match value {
            FunctionMemoryError::MemoryNotFound | FunctionMemoryError::MemoryNotMemory => Self::BadRequestFailedToAccessMemory,
        }
    }
}

impl From<FunctionMemoryAccessError> for BlobGetResponse {
    fn from(value: FunctionMemoryAccessError) -> Self {
        match value {
            FunctionMemoryAccessError::OutOfBounds => Self::BadRequestArgumentOutOfBounds,
        }
    }
}

impl From<FunctionMemoryGetStringError> for BlobGetResponse {
    fn from(value: FunctionMemoryGetStringError) -> Self {
        match value {
            FunctionMemoryGetStringError::OutOfBounds => Self::BadRequestArgumentOutOfBounds,
            FunctionMemoryGetStringError::FailedToDecode => Self::BadRequestArgumentFailedToDecode,
        }
    }
}
