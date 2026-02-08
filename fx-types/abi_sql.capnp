@0xfa46e9b349ec520d;

struct SqlExecRequest {
    binding @0 :Text;
    statement @1 :Text;
    params @2 :List(SqlValue);
}

struct SqlExecResult {
    rows @0 :List(SqlResultRow);
}

struct SqlMigrateRequest {
    binding @0 :Text;
    migrations @1 :List(Text);
}

struct SqlValue {
    value :union {
        null @0 :Void;
        integer @1 :Int64;
        real @2 :Float64;
        text @3 :Text;
        blob @4 :Data;
    }
}

struct SqlResultRow {
    columns @0 :List(SqlValue);
}
