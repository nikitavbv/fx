use thiserror::Error;

#[derive(Error, Debug, Eq, PartialEq)]
pub enum FxError {
    #[error("deserialization error")]
    DeserializationError { reason: String },
}
