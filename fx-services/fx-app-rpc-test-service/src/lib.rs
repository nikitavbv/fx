use {
    fx::rpc,
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

#[rpc]
pub fn hello(req: RpcRequest) -> fx::Result<RpcResponse> {
    tracing::info!("hello from rpc service! {req:?}");
    Ok(RpcResponse { number: req.number * 2 })
}
