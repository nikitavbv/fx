use {
    std::collections::HashMap,
    serde::Serialize,
    axum::{Json, http::{Uri, HeaderMap}},
};

#[derive(Serialize)]
pub(crate) struct PostEndpointResponse {
    data: String,
    headers: HashMap<String, String>,
    url: String,
}

pub(crate) async fn post(url: Uri, headers: HeaderMap, body: String) -> Json<PostEndpointResponse> {
    Json(PostEndpointResponse {
        data: body,
        headers: headers.into_iter()
            .filter_map(|(header_name, header_value)| header_name.map(|header_name| {
                (header_name.to_string(), header_value.to_str().unwrap().to_owned())
            }))
            .collect(),
        url: url.to_string(),
    })
}
