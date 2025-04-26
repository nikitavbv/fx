use thiserror::Error;

#[derive(Error, Debug)]
pub enum FxDatabaseError {
    #[error("other database error")]
    Other,
}
