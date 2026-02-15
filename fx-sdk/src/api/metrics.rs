use {
    fx_types::{capnp, abi_metrics_capnp},
    crate::sys::{fx_metrics_counter_register, fx_metrics_counter_increment},
};

#[derive(Clone)]
pub struct Counter {
    metric_id: MetricId,
}

impl Counter {
    pub fn new(name: impl Into<String>) -> Self {
        Self::new_with_labels(name, Vec::new()).with_label_values(Vec::new())
    }

    pub fn new_with_labels(name: impl Into<String>, labels: Vec<String>) -> CounterTemplate {
        CounterTemplate::new(name, labels)
    }

    fn from_metric_id(metric_id: MetricId) -> Self {
        Self { metric_id }
    }

    pub fn increment(&self) {
        self.increment_by(1)
    }

    pub fn increment_by(&self, delta: u64) {
        unsafe { fx_metrics_counter_increment(self.metric_id.as_abi(), delta); }
    }
}

pub struct CounterTemplate {
    name: String,
    label_names: Vec<String>,
}

impl CounterTemplate {
    pub fn new(name: impl Into<String>, labels: Vec<String>) -> Self {
        Self {
            name: name.into(),
            label_names: labels,
        }
    }

    pub fn with_label_values(&self, values: Vec<String>) -> Counter {
        let mut message = capnp::message::Builder::new_default();
        let mut request = message.init_root::<abi_metrics_capnp::counter_register::Builder>();
        request.set_name(&self.name);

        let mut labels = request.init_labels(self.label_names.len() as u32);

        let label_values = self.label_names.iter().zip(values.into_iter()).collect::<Vec<_>>();
        for (index, (name, value)) in label_values.into_iter().enumerate() {
            let mut label = labels.reborrow().get(index as u32);
            label.set_name(name);
            label.set_value(value);
        }

        let message = capnp::serialize::write_message_to_words(&message);

        Counter::from_metric_id(MetricId::from_abi(unsafe { fx_metrics_counter_register(message.as_ptr() as u64, message.len() as u64) }))
    }
}

#[derive(Clone)]
struct MetricId {
    id: u64,
}

impl MetricId {
    fn from_abi(id: u64) -> Self {
        Self { id }
    }

    fn as_abi(&self) -> u64 {
        self.id
    }
}
