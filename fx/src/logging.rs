use {
    std::collections::HashMap,
    tracing::{Subscriber, Event, field::{Field, Visit}},
    tracing_subscriber::{Layer, layer},
    fx_core::{LogMessage, LogLevel},
    crate::sys,
};

pub struct FxLoggingLayer;

struct FieldVisitor<'a> {
    fields: &'a mut HashMap<String, String>,
}

impl<'a> Visit for FieldVisitor<'a> {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.fields.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

impl<S> Layer<S> for FxLoggingLayer where S: Subscriber {
    fn on_event(&self, event: &Event<'_>, _ctx: layer::Context<'_, S>) {
        let mut fields = HashMap::new();
        event.record(&mut FieldVisitor { fields: &mut fields });

        let metadata = event.metadata();
        let level = match *metadata.level() {
            tracing::Level::TRACE => LogLevel::Trace,
            tracing::Level::DEBUG => LogLevel::Debug,
            tracing::Level::INFO => LogLevel::Info,
            tracing::Level::WARN => LogLevel::Warn,
            tracing::Level::ERROR => LogLevel::Error,
        };

        // fields.insert("metadata".to_owned(), format!("{:?}", event.metadata()));
        let msg = LogMessage { level, fields };

        let msg = rmp_serde::to_vec(&msg).unwrap();
        unsafe { sys::log(msg.as_ptr() as i64, msg.len() as i64); }
    }
}
