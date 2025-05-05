use {
    std::time::Duration,
    fx::{rpc, FxCtx, SqlQuery, sleep, FetchRequest},
    fx_utils::database::{sqlx::{self, ConnectOptions, Row}, FxDatabaseConnectOptions},
};

#[rpc]
pub async fn simple(_ctx: &FxCtx, arg: u32) -> u32 {
    arg + 42
}

#[rpc]
pub async fn sql_simple(ctx: &FxCtx, _arg: ()) -> u64 {
    let database = ctx.sql("app");
    database.exec(SqlQuery::new("create table test_sql_simple (v integer not null)"));
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (42)"));
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (10)"));
    let res = database.exec(SqlQuery::new("select sum(v) from test_sql_simple")).rows[0].columns.get(0).unwrap().try_into().unwrap();
    database.exec(SqlQuery::new("drop table test_sql_simple"));
    res
}

#[rpc]
pub async fn sqlx(ctx: &FxCtx, _arg: ()) -> u64 {
    let database = ctx.sql("app");

    let connection = FxDatabaseConnectOptions::new(database)
        .connect()
        .await
        .unwrap();

    sqlx::query("create table test_sql_simple (v integer not null)")
        .execute(&connection)
        .await
        .unwrap();
    sqlx::query("insert into test_sql_simple (v) values (42)")
        .execute(&connection)
        .await
        .unwrap();
    sqlx::query("insert into test_sql_simple (v) values (10)")
        .execute(&connection)
        .await
        .unwrap();

    let res = sqlx::query("select sum(v) from test_sql_simple")
        .fetch_one(&connection)
        .await
        .map(|row| row.get(0))
        .unwrap();

    sqlx::query("drop table test_sql_simple")
        .execute(&connection)
        .await
        .unwrap();

    res
}

#[rpc]
pub async fn async_simple(_ctx: &FxCtx, arg: u64) -> u64 {
    sleep(Duration::from_secs(3)).await;
    arg
}

#[rpc]
pub async fn rpc_responder(ctx: &FxCtx, arg: u64) -> u64 {
    ctx.init_logger();
    sleep(Duration::from_secs(1)).await;
    arg * 2
}

#[rpc]
pub async fn call_rpc(ctx: &FxCtx, arg: u64) -> u64 {
    ctx.init_logger();
    ctx.rpc("other-app", "rpc_responder", arg).await
}

#[rpc]
pub async fn test_fetch(ctx: &FxCtx, _arg: ()) -> String {
    ctx.init_logger();
    let response = ctx.fetch(
        FetchRequest::get("https://fx.nikitavbv.com/api/mock/get")
    ).await;

    tracing::info!("headers: {:?}", response.headers());

    String::from_utf8(response.body).unwrap()
}
