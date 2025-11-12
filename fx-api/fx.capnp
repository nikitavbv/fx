@0xbdf4f7ba405d8392;

struct FxApiCall {
    op :union {
        metricsCounterIncrement @0 :MetricsCounterIncrementRequest;
        rpc @1 :RpcCallRequest;
    }
}

struct FxApiCallResult {
    op :union {
        metricsCounterIncrement @0 :Void;
        rpc @1 :RpcCallResponse;
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
