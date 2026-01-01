use {
    std::sync::Mutex,
    fx_runtime::logs::{Logger, LogMessageEvent},
};

pub struct TestLogger {
    logs: Mutex<Vec<LogMessageEvent>>,
}

impl TestLogger {
    pub fn new() -> Self {
        Self {
            logs: Mutex::new(Vec::new()),
        }
    }

    pub fn events(&self) -> Vec<LogMessageEvent> {
        self.logs.lock().unwrap().clone()
    }
}

impl Logger for TestLogger {
    fn log(&self, message: LogMessageEvent) {
        self.logs.lock().unwrap().push(message);
    }
}
