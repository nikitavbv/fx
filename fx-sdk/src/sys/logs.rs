use {
    std::collections::HashMap,
    fx_types::{capnp, abi_log_capnp},
    fx_common::{LogLevel, LogEventType},
    crate::sys::fx_log,
};

pub(crate) fn log(event_type: LogEventType, level: LogLevel, fields: HashMap<String, String>) {
    let mut message = capnp::message::Builder::new_default();
    let mut request = message.init_root::<abi_log_capnp::log_message::Builder>();

    request.set_event_type(match event_type {
        LogEventType::Begin => abi_log_capnp::EventType::Begin,
        LogEventType::End => abi_log_capnp::EventType::End,
        LogEventType::Instant => abi_log_capnp::EventType::Instant,
    });

    request.set_level(match level {
        LogLevel::Trace => abi_log_capnp::LogLevel::Trace,
        LogLevel::Debug => abi_log_capnp::LogLevel::Debug,
        LogLevel::Info => abi_log_capnp::LogLevel::Info,
        LogLevel::Warn => abi_log_capnp::LogLevel::Warn,
        LogLevel::Error => abi_log_capnp::LogLevel::Error,
    });

    let mut request_fields = request.init_fields(fields.len() as u32);
    for (field_index, (field_name, field_value)) in fields.into_iter().enumerate() {
        let mut request_field = request_fields.reborrow().get(field_index as u32);
        request_field.set_name(field_name);
        request_field.set_value(field_value);
    }

    let message = capnp::serialize::write_message_segments_to_words(&message);
    unsafe { fx_log(message.as_ptr() as i64, message.len() as i64) }
}
