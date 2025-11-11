use {
    fx_api::{capnp, fx_capnp},
    crate::sys::invoke_fx_api,
};

#[derive(Clone)]
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
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let mut metrics_counter_increment_request = request.init_metrics_counter_increment();
        metrics_counter_increment_request.set_counter_name(&self.name);
        metrics_counter_increment_request.set_delta(delta);
        invoke_fx_api(message);
    }
}
