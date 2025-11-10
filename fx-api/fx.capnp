@0xbdf4f7ba405d8392;

struct FxApiCall {
    metricsCounterIncrement @0 :MetricsCounterIncrementRequest;
}

struct MetricsCounterIncrementRequest {
    counterName @0 :Text;
    delta @1 :UInt64;
}
