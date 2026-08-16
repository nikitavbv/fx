use {
    thiserror::Error,
    fx_types::abi::EnvGetResult,
    crate::sys::{fx_env_len, fx_env_get},
};

#[derive(Debug, Error)]
pub enum EnvGetError {
    #[error("internal sdk error")]
    InternalSdkError,
}

pub fn get(key: impl AsRef<str>) -> Result<Option<String>, EnvGetError> {
    let key = key.as_ref();

    let len = unsafe { fx_env_len(key.as_ptr() as u64, key.len() as u64) };
    if len < 0 {
        return Ok(None);
    }

    let value = unsafe {
        let result: Vec<u8> = vec![0; len as usize];

        let read_result = EnvGetResult::try_from(fx_env_get(key.as_ptr() as u64, key.len() as u64, result.as_ptr() as u64) as i64);
        let read_result = match read_result {
            Ok(v) => v,
            Err(_) => return Err(EnvGetError::InternalSdkError),
        };

        match read_result {
            EnvGetResult::Ok => result,
            EnvGetResult::BadRequest
            | EnvGetResult::FailedToReadRequest
            | EnvGetResult::NotFound
            | EnvGetResult::FailedToWriteValue => return Err(EnvGetError::InternalSdkError),
        }
    };

    Ok(Some(String::from_utf8(value).unwrap()))
}
