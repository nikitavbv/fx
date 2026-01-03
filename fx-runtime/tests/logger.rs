use {
    std::sync::Mutex,
    fx_runtime::logs::{Logger, LogMessageEvent, StdoutLogger},
};

pub struct TestLogger {
    logs: Mutex<Vec<LogMessageEvent>>,
    inner: StdoutLogger,
}

impl TestLogger {
    pub fn new() -> Self {
        Self {
            logs: Mutex::new(Vec::new()),
            inner: StdoutLogger::new(),
        }
    }

    pub fn events(&self) -> Vec<LogMessageEvent> {
        self.logs.lock().unwrap().clone()
    }
}

impl Logger for TestLogger {
    fn log(&self, message: LogMessageEvent) {
        self.inner.log(message.clone());
        self.logs.lock().unwrap().push(message);
    }
}
