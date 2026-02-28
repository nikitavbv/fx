#[derive(Clone, Debug)]
pub(crate) struct FunctionHttpListener {
    pub(crate) host: Option<String>,
}

pub(crate) struct CronTrigger {
    pub(crate) id: String,
    pub(crate) schedule: cron::Schedule,
}
