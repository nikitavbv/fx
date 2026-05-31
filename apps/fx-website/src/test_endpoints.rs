use {
    serde::Serialize,
    axum::{Json, http::Uri},
};

#[derive(Serialize)]
pub(crate) struct PostEndpointResponse {
    data: String,
    url: String,
}

pub(crate) async fn post(url: Uri, body: String) -> Json<PostEndpointResponse> {
    Json(PostEndpointResponse {
        data: body,
        url: url.to_string(),
    })
}
