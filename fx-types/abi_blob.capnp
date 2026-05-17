@0xa397c31dc0b3ff8c;

struct BlobGetResponse {
    response :union {
        notFound @0 :Void;
        value @1 :Data;
        bindingNotExists @2 :Void;
        badRequestArgumentOutOfBounds @3 :Void;
        badRequestArgumentFailedToDecode @4 :Void;
        badRequestFailedToAccessMemory @5 :Void;
    }
}
