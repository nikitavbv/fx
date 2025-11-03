use crate::sys::metrics_counter_increment;

pub struct Counter {
    name: String,
}

impl Counter {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
        }
    }

    pub fn increment(&self, delta: u64) {
        unsafe { metrics_counter_increment(self.name.as_ptr() as i64, self.name.len() as i64, delta as i64); }
    }
}
