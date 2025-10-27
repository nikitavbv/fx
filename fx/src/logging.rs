use {
    std::collections::HashMap,
    tracing::{Subscriber, Event, field::{Field, Visit}, span::Attributes, Id},
    tracing_subscriber::{Layer, layer},
    fx_common::{LogMessage, LogLevel},
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

impl<S> Layer<S> for FxLoggingLayer where S: Subscriber + for<'lookup> tracing_subscriber::registry::LookupSpan<'lookup> {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: layer::Context<'_, S>) {
        let span = ctx.span(id).unwrap();
        let mut fields = HashMap::<String, String>::new();
        let mut visitor = FieldVisitor { fields: &mut fields };
        attrs.record(&mut visitor);
        span.extensions_mut().insert(fields);
    }

    fn on_event(&self, event: &Event<'_>, ctx: layer::Context<'_, S>) {
        let mut fields = HashMap::new();

        if let Some(scope) = ctx.event_scope(event) {
            for span in scope.from_root() {
                if let Some(span_fields) = span.extensions().get::<HashMap<String, String>>() {
                    fields.extend(span_fields.clone());
                }
            }
        }

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
