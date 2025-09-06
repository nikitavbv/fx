use {
    std::collections::HashMap,
    serde::{Serialize, Deserialize},
    crate::{LogMessageEvent, LogSource},
};

#[derive(Serialize, Deserialize)]
pub struct FunctionInvokeEvent {
    pub request_id: Option<String>,
    pub timings: InvocationTimings,
}

#[derive(Serialize, Deserialize)]
pub struct InvocationTimings {
    pub total_time_millis: u64,
}

impl Into<LogMessageEvent> for FunctionInvokeEvent {
    fn into(self) -> LogMessageEvent {
        LogMessageEvent::new(LogSource::FxRuntime, HashMap::new())
    }
}
