use fx::{rpc, FxCtx};

#[rpc]
pub fn simple(_ctx: &FxCtx, arg: u32) -> u32 {
    arg + 42
}
