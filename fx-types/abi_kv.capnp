@0xd04d36b508d09e63;

struct KvSetResponse {
    response :union {
        ok @0 :Void;
        alreadyExists @1 :Void;
        runtimeShutdown @2 :Void;
        bindingNotFound @3 :Void;
        failedToReadRequest @4 :Void;
        badRequest @5 :Void;
    }
}

struct KvGetResponse {
    response :union {
        keyNotFound @0 :Void;
        value @1 :Data;
        runtimeShutdown @2 :Void;
        bindingNotFound @3 :Void;
        badRequest @4 :Void;
        failedToReadRequest @5 :Void;
    }
}

struct KvSubscriptionFrame {
    frame :union {
        streamEnd @0 :Void;
        bytes @1 :Data;
    }
}

struct KvPublishResult {
    result :union {
        ok @0 :Void;
        runtimeShutdown @1 :Void;
        bindingNotFound @2 :Void;
        badRequest @3 :Void;
        failedToReadRequest @4 :Void;
    }
}

struct KvDelexResult {
    result :union {
        ok @0 :Void;
        runtimeShutdown @1 :Void;
        badRequest @2 :Void;
        failedToReadRequest @3 :Void;
        resourceNotFound @4 :Void;
        bindingNotFound @5 :Void;
    }
}
