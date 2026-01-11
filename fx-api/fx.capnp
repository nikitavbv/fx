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
        futurePoll @12 :FuturePollRequest;
        futureDrop @13 :FutureDropRequest;
        streamExport @14 :StreamExportRequest;
        streamPollNext @15 :StreamPollNextRequest;
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
        futurePoll @12 :FuturePollResponse;
        futureDrop @13 :FutureDropResponse;
        streamExport @14 :StreamExportResponse;
        streamPollNext @15 :StreamPollNextResponse;
    }
}

struct FxFunctionApiCall {
    op :union {
        futurePoll @0 :FunctionFuturePollRequest;
        futureDrop @1 :FunctionFutureDropRequest;
        streamPollNext @2 :FunctionStreamPollNextRequest;
        streamDrop @3 :FunctionStreamDropRequest;
        invoke @4 :FunctionInvokeRequest;
    }
}

struct FxFunctionApiCallResult {
    op :union {
        futurePoll @0 :FunctionFuturePollResponse;
        futureDrop @1 :FunctionFutureDropResponse;
        streamPollNext @2 :FunctionStreamPollNextResponse;
        streamDrop @3 :FunctionStreamDropResponse;
        invoke @4 :FunctionInvokeResponse;
    }
}

struct MetricsCounterIncrementRequest {
    counterName @0 :Text;
    delta @1 :UInt64;
    tags @2 :List(MetricsTag);
}

struct MetricsTag {
    name @0 :Text;
    value @1 :Text;
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

struct FuturePollRequest {
    futureId @0 :UInt64;
}

struct FuturePollResponse {
    response :union {
        pending @0 :Void;
        result @1 :Data;
        error @2 :Text;
    }
}

struct FutureDropRequest {
    futureId @0 :UInt64;
}

struct FutureDropResponse {}

struct StreamExportRequest {
}

struct StreamExportResponse {
    response :union {
        streamId @0 :UInt64;
        error @1 :Text;
    }
}

struct StreamPollNextRequest {
    streamId @0 :UInt64;
}

struct StreamPollNextResponse {
    response :union {
        pending @0 :Void;
        ready @1 :StreamItem;
        finished @2 :Void;
    }

    struct StreamItem {
        item :union {
            result @0 :Data;
            error @1 :Text;
        }
    }
}

struct FunctionFuturePollRequest {
    futureId @0 :UInt64;
}

struct FunctionFuturePollResponse {
    response :union {
        pending @0 :Void;
        ready @1 :Data;
        error @2 :FunctionFuturePollError;
    }
}

struct FunctionFuturePollError {
    error :union {
        apiError @0 :Text;
        internalRuntimeError @1 :Void;
    }
}

struct FunctionFutureDropRequest {
    futureId @0 :UInt64;
}

struct FunctionFutureDropResponse {
    response :union {
        ok @0 :Void;
        error @1 :Void;
    }
}

struct FunctionStreamPollNextRequest {
    streamId @0 :UInt64;
}

struct FunctionStreamPollNextResponse {
    response :union {
        pending @0 :Void;
        ready @1 :Data;
        finished @2 :Void;
    }
}

struct FunctionStreamDropRequest {
    streamId @0 :UInt64;
}

struct FunctionStreamDropResponse {}

struct FunctionInvokeRequest {
    method @0 :Text;
    payload @1 :Data;
}

struct FunctionInvokeResponse {
    result :union {
        futureId @0 :UInt64;
        error @1 :FunctionInvokeError;
    }
}

struct FunctionInvokeError {
    error :union {
        badRequest @0 :Void;
        handlerNotFound @1 :Void;
        internalRuntimeError @2 :Void;
    }
}
