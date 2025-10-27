use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
    crate::{LogMessageEvent, LogSource, LogEventType, utils::object_to_event_fields},
};

#[derive(Serialize, Deserialize)]
pub struct FunctionInvokeEvent {
    pub request_id: Option<String>,
    pub timings: InvocationTimings,
    pub compiler_backend: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct InvocationTimings {
    pub total_time_millis: u64,
}

impl Into<LogMessageEvent> for FunctionInvokeEvent {
    fn into(self) -> LogMessageEvent {
        LogMessageEvent::new(LogSource::FxRuntime, LogEventType::Instant, object_to_event_fields(self).unwrap_or(HashMap::new()))
    }
}
