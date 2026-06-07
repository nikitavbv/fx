use {
    std::collections::HashMap,
    serde::Serialize,
    axum::{http::{Uri, HeaderMap, HeaderValue, header}, response::IntoResponse},
};

pub(crate) mod get {
    use super::*;

    #[derive(Serialize)]
    pub(crate) struct Response {
        headers: HashMap<String, String>,
        url: String,
    }

    pub(crate) async fn handler(url: Uri, headers: HeaderMap) -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            serde_json::to_string_pretty(&Response {
                url: request_url(url, &headers),
                headers: headers_into_map(headers),
            }).unwrap(),
        ).into_response()
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

    pub(crate) async fn handler(url: Uri, headers: HeaderMap, body: String) -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            serde_json::to_string_pretty(&Response {
                data: body,
                url: request_url(url, &headers),
                headers: headers_into_map(headers),
            }).unwrap(),
        ).into_response()
    }
}

pub(crate) mod headers {
    use super::*;

    #[derive(Serialize)]
    pub(crate) struct Response {
        headers: HashMap<String, String>,
    }

    pub(crate) async fn handler(headers: HeaderMap) -> impl IntoResponse {
        (
            [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))],
            serde_json::to_string_pretty(&Response {
                headers: headers_into_map(headers),
            }).unwrap(),
        ).into_response()
    }
}

fn headers_into_map(headers: HeaderMap) -> HashMap<String, String> {
    headers.into_iter()
        .filter_map(|(header_name, header_value)| header_name.map(|header_name| {
            (header_name.to_string(), header_value.to_str().unwrap().to_owned())
        }))
        .collect()
}

fn request_url(url: Uri, headers: &HeaderMap) -> String {
    format!(
        "{}{url}",
        headers.get("host")
            .or_else(|| headers.get(":authority"))
            .and_then(|v| v.to_str().ok())
            .unwrap_or("localhost")
            .to_owned()
    )
}
