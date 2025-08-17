use {fx::{FxCtx, rpc}, fx_runtime_common::FunctionInvokeEvent, crate::database::Database};

#[rpc]
pub async fn on_invoke(ctx: &FxCtx, event: FunctionInvokeEvent) {
    let database = ctx.sql("dashboard");
    let database = Database::new(database.clone()).await;
    database.run_migrations();
    database.function_invocations_incr(event.function_id);
}
