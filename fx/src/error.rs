use thiserror::Error;

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FxError {
    #[error("deserialization error")]
    DeserializationError { reason: String },

    #[error("future error: {reason}")]
    FutureError { reason: String },
}
