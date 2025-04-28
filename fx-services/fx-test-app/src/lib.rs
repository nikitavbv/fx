use fx::{rpc, FxCtx, SqlQuery};

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
