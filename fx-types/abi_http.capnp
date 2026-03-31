@0x85d77e32e23799d2;

struct HttpRequest {
    uri @0 :Text;
    method @1 :HttpMethod;
    headers @2 :List(HttpHeader);
    body @3 :HttpBody;
}

enum HttpMethod {
    get @0;
    post @1;
    put @2;
    patch @3;
    delete @4;
    options @5;
    head @6;
    connect @7;
    trace @8;
}

struct HttpHeader {
    name @0 :Text;
    value @1 :Text;
}

struct HttpBody {
    body :union {
        empty @0 :Void;
        bytes @1 :Data;
        hostResource @2 :UInt64;
        stream @3 :Void;
    }
}

struct HttpBodyFrame {
    frame :union {
        streamEnd @0 :Void;
        bytes @1 :Data;
    }
}

struct HttpResponse {
    status @0 :UInt16;
    headers @2 :List(HttpHeader);
    bodyResourceId @1 :UInt64;
}

struct FunctionHttpBodyFrame {
    body :union {
        streamEnd @0 :Void;
        bytes @1 :Data;
        hostResourceId @2 :UInt64;
    }
}

struct FunctionResponse {
    status @0 :UInt16;
    headers @2 :List(HttpHeader);

    bodyResource @1 :UInt64;
}
