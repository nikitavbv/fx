@0xfa5b3200271bb0b3;

struct LogMessage {
    level @0 :LogLevel;
    fields @1 :List(LogField);
    eventType @2 :EventType;
}

enum LogLevel {
    trace @0;
    debug @1;
    info @2;
    warn @3;
    error @4;
}

struct LogField {
    name @0 :Text;
    value @1 :Text;
}

enum EventType {
    begin @0;
    end @1;
    instant @2;
}
