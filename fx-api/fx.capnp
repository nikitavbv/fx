@0xbdf4f7ba405d8392;

struct FxApiCall {
    op :union {
        metricsCounterIncrement @0 :MetricsCounterIncrementRequest;
        rpc @1 :RpcCallRequest;
        kvGet @2 :KvGetRequest;
        kvSet @3 :KvSetRequest;
        sqlExec @4 :SqlExecRequest;
        sqlBatch @5 :SqlBatchRequest;
        sqlMigrate @6 :SqlMigrateRequest;
        log @7 :LogRequest;
        fetch @8 :FetchRequest;
        sleep @9 :SleepRequest;
        random @10 :RandomRequest;
        time @11 :TimeRequest;
    }
}

struct FxApiCallResult {
    op :union {
        metricsCounterIncrement @0 :Void;
        rpc @1 :RpcCallResponse;
        kvGet @2 :KvGetResponse;
        kvSet @3 :KvSetResponse;
        sqlExec @4 :SqlExecResponse;
        sqlBatch @5 :SqlBatchResponse;
        sqlMigrate @6 :SqlMigrateResponse;
        log @7 :LogResponse;
        fetch @8 :FetchResponse;
        sleep @9 :SleepResponse;
        random @10 :RandomResponse;
        time @11 :TimeResponse;
    }
}

struct MetricsCounterIncrementRequest {
    counterName @0 :Text;
    delta @1 :UInt64;
}

struct RpcCallRequest {
    functionId @0 :Text;
    methodName @1 :Text;
    argument @2 :Data;
}

struct RpcCallResponse {
    response :union {
        futureId @0 :UInt64;
        bindingNotFound @1 :Void;
        runtimeError @2 :Void;
    }
}

struct KvGetRequest {
    bindingId @0 :Text;
    key @1 :Data;
}

struct KvGetResponse {
    response :union {
        value @0 :Data;
        bindingNotFound @1 :Void;
        keyNotFound @2 :Void;
    }
}

struct KvSetRequest {
    bindingId @0 :Text;
    key @1 :Data;
    value @2 :Data;
}

struct KvSetResponse {
    response :union {
        ok @0 :Void;
        bindingNotFound @1 :Void;
    }
}

struct SqlExecRequest {
    database @0 :Text;
    query @1 :SqlQuery;
}

struct SqlQuery {
    statement @0 :Text;
    params @1 :List(SqlValue);
}

struct SqlValue {
    value :union {
        null @0 :Void;
        integer @1 :Int64;
        real @2 :Float64;
        text @3 :Text;
        blob @4 :Data;
    }
}

struct SqlExecResponse {
    response :union {
        rows @0 :List(SqlResultRow);
        bindingNotFound @1 :Void;
        sqlError @2 :SqlError;
    }
}

struct SqlResultRow {
    columns @0 :List(SqlValue);
}

struct SqlError {
    description @0 :Text;
}

struct SqlBatchRequest {
    database @0 :Text;
    queries @1 :List(SqlQuery);
}

struct SqlBatchResponse {
    response :union {
        ok @0 :Void;
        bindingNotFound @1 :Void;
        sqlError @2 :SqlError;
    }
}

struct SqlMigrateRequest {
    database @0 :Text;
    migrations @1 :List(Text);
}

struct SqlMigrateResponse {
    response :union {
        ok @0 :Void;
        bindingNotFound @1 :Void;
        sqlError @2 :SqlError;
    }
}

struct LogRequest {
    level @0 :LogLevel;
    fields @1 :List(LogField);
    eventType @2 :EventType;
}

enum LogLevel {
    trace @0;
    debug @1;
    info @2;
    warn @3;
    error @4;
}

struct LogField {
    name @0 :Text;
    value @1 :Text;
}

enum EventType {
    begin @0;
    end @1;
    instant @2;
}

struct LogResponse {}

struct FetchRequest {
    method @0 :HttpMethod;
    url @1 :Text;
    headers @2 :List(HttpHeader);
    body @3 :Stream;
}

enum HttpMethod {
    get @0;
    put @1;
    post @2;
    patch @3;
    delete @4;
}

struct HttpHeader {
    name @0 :Text;
    value @1 :Text;
}

struct Stream {
    id @0 :UInt64;
}

struct FetchResponse {
    response :union {
        futureId @0 :UInt64;
        fetchError @1 :Text;
    }
}

struct SleepRequest {
    millis @0 :UInt64;
}

struct SleepResponse {
    response :union {
        futureId @0 :UInt64;
        sleepError @1 :Text;
    }
}

struct RandomRequest {
    length @0 :UInt64;
}

struct RandomResponse {
    data @0 :Data;
}

struct TimeRequest {}

struct TimeResponse {
    timestamp @0 :UInt64;
}
