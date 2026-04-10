use crate::sys::{fx_env_len, fx_env_get};

pub fn get(key: impl AsRef<str>) -> Option<String> {
    let key = key.as_ref();

    let len = unsafe { fx_env_len(key.as_ptr() as u64, key.len() as u64) };

    (len >= 0).then(|| {
        String::from_utf8(unsafe {
            let result: Vec<u8> = vec![0; len as usize];
            fx_env_get(key.as_ptr() as u64, key.len() as u64, result.as_ptr() as u64);
            result
        }).unwrap()
    })
}
