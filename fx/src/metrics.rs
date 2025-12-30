use {
    fx_api::{capnp, fx_capnp},
    crate::sys::invoke_fx_api,
};

#[derive(Clone)]
pub struct Counter {
    name: String,
    tags: Vec<String>,
}

impl Counter {
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_tags(name, Vec::new())
    }

    pub fn new_with_tags(name: impl Into<String>, tags: Vec<String>) -> Self {
        Self {
            name: name.into(),
            tags,
        }
    }

    pub fn increment(&self, delta: u64) {
        self.increment_with_tag_values(Vec::new(), delta)
    }

    pub fn increment_with_tag_values(&self, values: Vec<String>, delta: u64) {
        let mut message = capnp::message::Builder::new_default();
        let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
        let op = request.init_op();
        let mut metrics_counter_increment_request = op.init_metrics_counter_increment();
        metrics_counter_increment_request.set_counter_name(&self.name);
        metrics_counter_increment_request.set_delta(delta);

        if !values.is_empty() {
            let kvs = self.tags.iter().zip(values.into_iter()).collect::<Vec<_>>();
            let mut request_tags = metrics_counter_increment_request.init_tags(kvs.len() as u32);

            for (index, (name, value)) in kvs.iter().enumerate() {
                let mut tag = request_tags.reborrow().get(index as u32);
                tag.set_name(name);
                tag.set_value(value);
            }
        }

        invoke_fx_api(message);
    }
}
