use crate::{
    function::abi::{capnp, abi_blob_capnp, function_memory::{FunctionMemoryError, FunctionMemoryAccessError, FunctionMemoryGetStringError}},
    resources::serialize::SerializeResource,
};

pub(crate) enum BlobGetResponse {
    NotFound,
    Ok(Vec<u8>),
    BadRequestFailedToAccessMemory,
    BadRequestArgumentOutOfBounds,
    BadRequestArgumentFailedToDecode,
    BindingNotExists,
}

impl SerializeResource for BlobGetResponse {
    fn serialize(self) -> Vec<u8> {
        let mut message = capnp::message::Builder::new_default();
        let blob_get_response = message.init_root::<abi_blob_capnp::blob_get_response::Builder>();
        let mut response = blob_get_response.init_response();

        match self {
            Self::NotFound => response.set_not_found(()),
            Self::Ok(v) => response.set_value(&v),
            Self::BadRequestFailedToAccessMemory => response.set_bad_request_failed_to_access_memory(()),
            Self::BadRequestArgumentOutOfBounds => response.set_bad_request_argument_out_of_bounds(()),
            Self::BadRequestArgumentFailedToDecode => response.set_bad_request_argument_failed_to_decode(()),
            Self::BindingNotExists => response.set_binding_not_exists(()),
        }

        capnp::serialize::write_message_to_words(&message)
    }
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
