use std::time::Duration;

#[derive(Clone, Debug)]
pub(crate) struct FunctionHttpListener {
    pub(crate) host: Option<String>,
}

pub(crate) struct CronTrigger {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) schedule: cron::Schedule,
    pub(crate) endpoint: Option<String>,
    pub(crate) timeout: Option<Duration>,
}
