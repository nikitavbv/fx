@0xbdf4f7ba405d8392;

struct FxApiCall {
    op :union {
        metricsCounterIncrement @0 :MetricsCounterIncrementRequest;
        rpc @1 :RpcCallRequest;
        kvGet @2 :KvGetRequest;
        kvSet @3 :KvSetRequest;
    }
}

struct FxApiCallResult {
    op :union {
        metricsCounterIncrement @0 :Void;
        rpc @1 :RpcCallResponse;
        kvGet @2 :KvGetResponse;
        kvSet @3 :KvSetResponse;
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
