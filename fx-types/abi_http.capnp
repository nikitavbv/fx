@0x85d77e32e23799d2;

struct HttpRequest {
    uri @0 :Text;
    method @1 :HttpMethod;
    headers @2 :List(HttpHeader);
}

enum HttpMethod {
    get @0;
    post @1;
    put @2;
    patch @3;
    delete @4;
    options @5;
}

struct HttpHeader {
    name @0 :Text;
    value @1 :Text;
}

struct HttpResponse {
    status @0 :UInt16;
    body @1 :Data;
}
