use {
    thiserror::Error,
    fx_types::abi::{EnvLenResult, EnvGetResult, EnvLenResultCode},
    crate::sys::{fx_env_len, fx_env_get},
};

#[derive(Debug, Error)]
pub enum EnvGetError {
    #[error("internal sdk error")]
    InternalSdkError,
    #[error("internal sdk assertion error")]
    AssertionError,
}

pub fn get(key: impl AsRef<str>) -> Result<Option<String>, EnvGetError> {
    let key = key.as_ref();

    let mut result = std::mem::MaybeUninit::<EnvLenResult>::zeroed();
    let result_code = unsafe { fx_env_len(key.as_ptr() as u64, key.len() as u64, result.as_mut_ptr() as u64) };
    let result_code = EnvLenResultCode::try_from(result_code).map_err(|_| EnvGetError::InternalSdkError)?;
    match result_code {
        EnvLenResultCode::Ok => {},
        EnvLenResultCode::NotFound => return Ok(None),
        EnvLenResultCode::BadRequest
        | EnvLenResultCode::FailedToReadRequest => return Err(EnvGetError::InternalSdkError),
    }

    let result = unsafe { result.assume_init() };

    let value = unsafe {
        let result: Vec<u8> = vec![0; result.len as usize];

        let read_result = EnvGetResult::try_from(fx_env_get(key.as_ptr() as u64, key.len() as u64, result.as_ptr() as u64) as i64);
        let read_result = match read_result {
            Ok(v) => v,
            Err(_) => return Err(EnvGetError::InternalSdkError),
        };

        match read_result {
            EnvGetResult::Ok => result,
            EnvGetResult::NotFound => return Err(EnvGetError::AssertionError), // not found is checked above, so this should not be reachable
            EnvGetResult::BadRequest
            | EnvGetResult::FailedToReadRequest
            | EnvGetResult::FailedToWriteValue => return Err(EnvGetError::InternalSdkError),
        }
    };

    Ok(Some(String::from_utf8(value).unwrap()))
}
