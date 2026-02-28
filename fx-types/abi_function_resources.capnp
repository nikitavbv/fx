@0xe3f3e4f9813db34e;

# TODO: move to abi_http
struct FunctionResponse {
    status @0 :UInt16;
    headers @2 :List(HttpHeader);

    bodyResource @1 :UInt64;
}

struct HttpHeader {
    name @0 :Text;
    value @1 :Text;
}
