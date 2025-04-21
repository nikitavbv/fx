use {
    fx::{FxCtx, rpc},
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
pub fn hello(ctx: &FxCtx, req: RpcRequest) -> RpcResponse {
    ctx.init_logger();
    tracing::info!("hello from rpc service! {req:?}");
    RpcResponse { number: req.number * 2 }
}
