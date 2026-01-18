use {
    fx_sdk::{self as fx, handler},
    serde::{Serialize, Deserialize},
};

#[derive(Deserialize, Debug)]
pub struct RpcRequest {
    number: i64,
}

#[derive(Serialize)]
pub struct RpcResponse {
    number: i64,
}

#[handler]
pub async fn hello(req: RpcRequest) -> fx::Result<RpcResponse> {
    tracing::info!("hello from rpc service! {req:?}");
    Ok(RpcResponse { number: req.number * 2 })
}
