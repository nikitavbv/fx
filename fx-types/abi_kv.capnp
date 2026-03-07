@0xd04d36b508d09e63;

struct KvGetResponse {
    response :union {
        keyNotFound @0 :Void;
        value @1 :Data;
    }
}
