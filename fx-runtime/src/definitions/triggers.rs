#[derive(Clone, Debug)]
pub(crate) struct FunctionHttpListener {
    pub(crate) host: Option<String>,
}

impl FunctionHttpListener {
    fn new(host: Option<String>) -> Self {
        Self {
            host,
        }
    }
}

pub(crate) struct CronTrigger {
    pub(crate) id: String,
    pub(crate) schedule: cron::Schedule,
}
