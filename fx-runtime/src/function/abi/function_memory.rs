use {
    thiserror::Error,
    crate::function::instance::FunctionInstanceState,
};

pub(crate) struct FunctionMemory {
    memory: wasmtime::Memory,
}

impl FunctionMemory {
    pub(crate) fn from_caller(caller: &mut wasmtime::Caller<'_, FunctionInstanceState>) -> Result<Self, FunctionMemoryError> {
        caller.get_export("memory")
            .ok_or(FunctionMemoryError::MemoryNotFound)
            .and_then(|v| v.into_memory().ok_or(FunctionMemoryError::MemoryNotMemory))
            .map(|memory| Self { memory })
    }

    pub(crate) fn view<'a>(&'a self, context: &'a wasmtime::StoreContext<'_, FunctionInstanceState>) -> FunctionMemoryView<'a> {
        FunctionMemoryView {
            view: self.memory.data(context),
        }
    }

    pub(crate) fn view_mut<'a>(&'a self, context: &'a mut wasmtime::StoreContextMut<'_, FunctionInstanceState>) -> FunctionMemoryViewMut<'a> {
        FunctionMemoryViewMut {
            view: self.memory.data_mut(context),
        }
    }
}

pub(crate) struct FunctionMemoryView<'a> {
    view: &'a [u8],
}

impl<'a> FunctionMemoryView<'a> {
    pub(crate) fn slice(&self, ptr: u64, len: u64) -> Result<&[u8], FunctionMemoryAccessError> {
        let ptr = ptr as usize;
        let len = len as usize;
        self.view.get(ptr..ptr+len).ok_or(FunctionMemoryAccessError::OutOfBounds)
    }

    pub(crate) fn vec_clone(&self, ptr: u64, len: u64) -> Result<Vec<u8>, FunctionMemoryAccessError> {
        self.slice(ptr, len).map(|v| v.to_vec())
    }

    pub(crate) fn str_ref(&self, ptr: u64, len: u64) -> Result<&str, FunctionMemoryGetStringError> {
        self.slice(ptr, len)
            .map_err(FunctionMemoryGetStringError::from)
            .and_then(|v| str::from_utf8(v).map_err(|_| FunctionMemoryGetStringError::FailedToDecode))
    }
}

pub(crate) struct FunctionMemoryViewMut<'a> {
    view: &'a mut [u8],
}

impl<'a> FunctionMemoryViewMut<'a> {
    pub(crate) fn copy_from_slice(&mut self, ptr: u64, len: u64, copy_from: &[u8]) -> Result<(), FunctionMemoryAccessError> {
        let ptr = ptr as usize;
        let len = len as usize;
        self.view.get_mut(ptr..ptr+len).ok_or(FunctionMemoryAccessError::OutOfBounds)?.copy_from_slice(copy_from);
        Ok(())
    }
}

#[derive(Debug, Error)]
pub(crate) enum FunctionMemoryError {
    #[error("memory export not found")]
    MemoryNotFound,

    #[error("exported memory is not a memory")]
    MemoryNotMemory,
}

#[derive(Debug, Error)]
pub(crate) enum FunctionMemoryAccessError {
    #[error("index into function memory is out of bounds of memory view")]
    OutOfBounds,
}

#[derive(Debug, Error)]
pub(crate) enum FunctionMemoryGetStringError {
    #[error("index into function memory is out of bounds of memory view")]
    OutOfBounds,

    #[error("failed to decode string as utf8")]
    FailedToDecode,
}

impl From<FunctionMemoryAccessError> for FunctionMemoryGetStringError {
    fn from(err: FunctionMemoryAccessError) -> Self {
        match err {
            FunctionMemoryAccessError::OutOfBounds => Self::OutOfBounds,
        }
    }
}
