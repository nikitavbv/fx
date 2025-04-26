use {fx::{FxCtx, rpc, SqlQuery}, fx_cloud_common::FunctionInvokeEvent};

#[rpc]
pub fn on_invoke(ctx: &FxCtx, event: FunctionInvokeEvent) {
    let database = ctx.sql("dashboard");
    database.exec(SqlQuery::new("insert into functions (function_id, total_invocations) values (?, 1) on conflict (function_id) do update set total_invocations = total_invocations + 1").bind(event.function_id));
}
