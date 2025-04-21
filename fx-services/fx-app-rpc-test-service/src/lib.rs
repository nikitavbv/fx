use {
    fx::{FxCtx, rpc},
    bincode::{Encode, Decode},
};

#[derive(Decode, Debug)]
pub struct RpcRequest {
    number: i64,
}

#[derive(Encode)]
pub struct RpcResponse {
    number: i64,
}

#[rpc]
pub fn hello(ctx: &FxCtx, req: RpcRequest) -> RpcResponse {
    ctx.init_logger();
    tracing::info!("hello from rpc service! {req:?}");
    RpcResponse { number: req.number * 2 }
}
