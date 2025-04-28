use {
    fx::{rpc, FxCtx, SqlQuery},
    fx_utils::{database::{sqlx::{self, ConnectOptions, Row}, FxDatabaseConnectOptions}, block_on},
};

#[rpc]
pub fn simple(_ctx: &FxCtx, arg: u32) -> u32 {
    arg + 42
}

#[rpc]
pub fn sql_simple(ctx: &FxCtx, _arg: ()) -> u64 {
    let database = ctx.sql("app");
    database.exec(SqlQuery::new("create table test_sql_simple (v integer not null)"));
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (42)"));
    database.exec(SqlQuery::new("insert into test_sql_simple (v) values (10)"));
    let res = database.exec(SqlQuery::new("select sum(v) from test_sql_simple")).rows[0].columns.get(0).unwrap().try_into().unwrap();
    database.exec(SqlQuery::new("drop table test_sql_simple"));
    res
}

#[rpc]
pub fn sqlx(ctx: &FxCtx, _arg: ()) -> u64 {
    let database = ctx.sql("app");

    block_on(async {
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
    })
}
