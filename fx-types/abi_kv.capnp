@0xd04d36b508d09e63;

struct KvSetResponse {
    response :union {
        ok @0 :Void;
        alreadyExists @1 :Void;
    }
}

struct KvGetResponse {
    response :union {
        keyNotFound @0 :Void;
        value @1 :Data;
    }
}
