@0xbdf4f7ba405d8392;

struct FxApiCall {
    op :union {
        metricsCounterIncrement @0 :MetricsCounterIncrementRequest;
        rpc @1 :RpcCallRequest;
        kvGet @2 :KvGetRequest;
        kvSet @3 :KvSetRequest;
        sqlExec @4 :SqlExecRequest;
    }
}

struct FxApiCallResult {
    op :union {
        metricsCounterIncrement @0 :Void;
        rpc @1 :RpcCallResponse;
        kvGet @2 :KvGetResponse;
        kvSet @3 :KvSetResponse;
        sqlExec @4 :SqlExecResponse;
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
    rows @0 :List(SqlResultRow);
}

struct SqlResultRow {
    columns @0 :List(SqlValue);
}
