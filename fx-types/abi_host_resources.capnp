@0xe24cc8edaa927596;

struct FunctionRequest {
    uri @0 :Text;
    method @1 :HttpMethod;
}

enum HttpMethod {
    get @0;
    post @1;
    put @2;
    patch @3;
    delete @4;
    options @5;
}
