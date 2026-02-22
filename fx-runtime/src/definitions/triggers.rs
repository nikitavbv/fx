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
