#[derive(Clone)]
pub(crate) struct FunctionStreamResourceId {
    id: u64,
}

impl FunctionStreamResourceId {
    pub fn as_u64(&self) -> u64 {
        self.id
    }
}

impl From<u64> for FunctionStreamResourceId {
    fn from(id: u64) -> Self {
        Self { id }
    }
}
