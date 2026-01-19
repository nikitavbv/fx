@0x8bd833c0f990e709;

struct FxLogEvent {
    source @0 :LogSource;
    eventType @1 :LogEventType;
    level @2 :LogLevel;
    # TODO: add fields
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
