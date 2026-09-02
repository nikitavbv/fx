@0xa397c31dc0b3ff8c;

struct BlobPutRequest {
    binding @0 :Text;
    key @1 :Data;
    value @2 :Data;
}

struct BlobPutResult {
    result :union {
        ok @0 :Void;
        storageError @1 :Void;
    }
}

struct BlobGetRequest {
    binding @0 :Text;
    key @1 :Data;
}

struct BlobGetResponse {
    response :union {
        notFound @0 :Void;
        value @1 :Data;
        bindingNotExists @2 :Void;
        badRequestArgumentOutOfBounds @3 :Void;
        badRequestFailedToAccessMemory @4 :Void;
        storageError @5 :Void;
    }
}

struct BlobDeleteRequest {
    binding @0 :Text;
    key @1 :Data;
}

struct BlobDeleteResponse {
    response :union {
        ok @0 :Void;
        storageError @1 :Void;
    }
}
