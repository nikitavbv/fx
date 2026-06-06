use {
    std::collections::HashMap,
    serde::Serialize,
    axum::{Json, http::{Uri, HeaderMap}},
};

pub(crate) mod get {
    use super::*;

    #[derive(Serialize)]
    pub(crate) struct Response {
        headers: HashMap<String, String>,
        url: String,
    }

    pub(crate) async fn handler(url: Uri, headers: HeaderMap) -> Json<Response> {
        Json(Response {
            headers: headers_into_map(headers),
            url: url.to_string(),
        })
    }
}

pub(crate) mod post {
    use super::*;

    #[derive(Serialize)]
    pub(crate) struct Response {
        data: String,
        headers: HashMap<String, String>,
        url: String,
    }

    pub(crate) async fn handler(url: Uri, headers: HeaderMap, body: String) -> Json<Response> {
        Json(Response {
            data: body,
            headers: headers_into_map(headers),
            url: url.to_string(),
        })
    }
}

fn headers_into_map(headers: HeaderMap) -> HashMap<String, String> {
    headers.into_iter()
        .filter_map(|(header_name, header_value)| header_name.map(|header_name| {
            (header_name.to_string(), header_value.to_str().unwrap().to_owned())
        }))
        .collect()
}
