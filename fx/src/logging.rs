use {
    std::collections::HashMap,
    tracing::{Subscriber, Event, field::{Field, Visit}},
    tracing_subscriber::{Layer, layer},
    fx_core::LogMessage,
    crate::sys,
};

pub struct FxLoggingLayer;

struct FieldVisitor<'a> {
    fields: &'a mut HashMap<String, String>,
}

impl<'a> Visit for FieldVisitor<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(field.name().to_owned(), format!("{:?}", value));
    }
}

impl<S> Layer<S> for FxLoggingLayer where S: Subscriber {
    fn on_event(&self, event: &Event<'_>, _ctx: layer::Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldVisitor { fields: &mut fields });
        let msg = LogMessage { fields };
        let msg = bincode::encode_to_vec(&msg, bincode::config::standard()).unwrap();
        unsafe { sys::log(msg.as_ptr() as i64, msg.len() as i64); }
    }
}
