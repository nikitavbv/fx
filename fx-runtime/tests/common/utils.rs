use fx_runtime::server::RunningFxServer;

pub struct TestClient {
    client: reqwest::Client,
    base_url: String,
    introspection_url: String,
}

impl TestClient {
    pub fn new(base_url: String, introspection_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
            introspection_url,
        }
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.get(&url)
    }

    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.post(&url)
    }

    pub fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.delete(&url)
    }

    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.request(method, &url)
    }

    pub fn introspection_get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client.get(format!("{}{}", self.introspection_url, path))
    }
}

pub struct TestServer {
    #[allow(dead_code)]
    pub server: RunningFxServer,
    pub base_url: String,
    pub introspection_url: String,
}
