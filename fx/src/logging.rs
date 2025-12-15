use {
    std::collections::HashMap,
    tracing::{Subscriber, Event, field::{Field, Visit}, span::Attributes, Id},
    tracing_subscriber::{Layer, layer},
    fx_common::{LogMessage, LogLevel, LogEventType},
    fx_api::{capnp, fx_capnp},
    crate::invoke_fx_api,
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
        span.extensions_mut().insert(fields.clone());
    }

    fn on_enter(&self, id: &Id, ctx: layer::Context<'_, S>) {
        let span = ctx.span(id).expect("span must exist");
        log(
            LogEventType::Begin,
            log_level_from_metadata(span.metadata()),
            {
                let mut fields = span.extensions().get::<HashMap<String, String>>().cloned().unwrap_or(HashMap::new());
                fields.insert("name".to_owned(), span.name().to_owned());
                fields
            }
        );
    }

    fn on_exit(&self, id: &Id, ctx: layer::Context<'_, S>) {
        let span = ctx.span(id).expect("span must exist");
        log(
            LogEventType::End,
            log_level_from_metadata(span.metadata()),
            {
                let mut fields = span.extensions().get::<HashMap<String, String>>().cloned().unwrap_or(HashMap::new());
                fields.insert("name".to_owned(), span.name().to_owned());
                fields
            }
        );
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

        log(LogEventType::Instant, log_level_from_metadata(event.metadata()), fields);
    }
}

fn log_level_from_metadata(metadata: &'static tracing::Metadata<'static>) -> LogLevel {
    match *metadata.level() {
        tracing::Level::TRACE => LogLevel::Trace,
        tracing::Level::DEBUG => LogLevel::Debug,
        tracing::Level::INFO => LogLevel::Info,
        tracing::Level::WARN => LogLevel::Warn,
        tracing::Level::ERROR => LogLevel::Error,
    }
}

fn log(event_type: LogEventType, level: LogLevel, fields: HashMap<String, String>) {
    let mut message = capnp::message::Builder::new_default();
    let request = message.init_root::<fx_capnp::fx_api_call::Builder>();
    let op = request.init_op();
    let mut log_request = op.init_log();

    log_request.set_event_type(match event_type {
        LogEventType::Begin => fx_capnp::EventType::Begin,
        LogEventType::End => fx_capnp::EventType::End,
        LogEventType::Instant => fx_capnp::EventType::Instant,
    });

    log_request.set_level(match level {
        LogLevel::Trace => fx_capnp::LogLevel::Trace,
        LogLevel::Debug => fx_capnp::LogLevel::Debug,
        LogLevel::Info => fx_capnp::LogLevel::Info,
        LogLevel::Warn => fx_capnp::LogLevel::Warn,
        LogLevel::Error => fx_capnp::LogLevel::Error,
    });

    let mut request_fields = log_request.init_fields(fields.len() as u32);
    for (field_index, (field_name, field_value)) in fields.into_iter().enumerate() {
        let mut request_field = request_fields.reborrow().get(field_index as u32);
        request_field.set_name(field_name);
        request_field.set_value(field_value);
    }
    let _response = invoke_fx_api(message);
}
