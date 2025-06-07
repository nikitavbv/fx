pub trait Queue {
    fn push(&self, message: &[u8]);
}

pub struct BoxStream {
    inner: Box<dyn Queue + Send + Sync>,
}

impl BoxStream {
    pub fn new<T: Queue + Send + Sync + 'static>(inner: T) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }
}
