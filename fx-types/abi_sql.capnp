@0xfa46e9b349ec520d;

struct SqlExecRequest {
    binding @0 :Text;
    statement @1 :Text;
    params @2 :List(SqlValue);
}

struct SqlExecResult {
    result :union {
        rows @0 :List(SqlResultRow);
        error @1 :SqlExecError;
    }
}

struct SqlExecError {
    error :union {
        databaseBusy @0 :Void;
        bindingNotFound @1 :Void;
    }
}

struct SqlMigrateRequest {
    binding @0 :Text;
    migrations @1 :List(Text);
}

struct SqlMigrateResult {
    result :union {
        ok @0 :Void;
        error @1 :SqlMigrateError;
    }
}

struct SqlMigrateError {
    error :union {
        databaseBusy @0 :Void;
        bindingNotFound @1 :Void;
        executionError @2 :Text;
        sqlError @3 :Text;
    }
}

struct SqlBatchRequest {
    binding @0 :Text;
    queries @1 :List(SqlBatchQuery);
}

struct SqlBatchQuery {
    statement @0 :Text;
    params @1 :List(SqlValue);
}

struct SqlBatchResult {
    result :union {
        ok @0 :Void;
        error @1 :SqlBatchError;
    }
}

struct SqlBatchError {
    error :union {
        databaseBusy @0 :Void;
        bindingNotFound @1 :Void;
        statementFailed @2 :Text;
    }
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
