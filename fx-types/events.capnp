@0x8bd833c0f990e709;

struct FxLogEvent {
    source @0 :LogSource;
    eventType @1 :LogEventType;
    level @2 :LogLevel;
    fields @3 :List(LogEventField);
}

struct LogSource {
    source :union {
        runtime @0 :Void;
        function @1 :Text;
    }
}

enum LogEventType {
    begin @0;
    end @1;
    instant @2;
}

enum LogLevel {
    trace @0;
    debug @1;
    info @2;
    warn @3;
    error @4;
}

struct LogEventField {
    key @0 :Text;
    value :union {
        text @1 :Text;
        u64 @2 :UInt64;
        i64 @3 :Int64;
        f64 @4 :Float64;
        object @5 :List(LogEventField);
    }
}
