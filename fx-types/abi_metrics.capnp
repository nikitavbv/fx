@0xb0923999303e7592;

struct CounterRegister {
    name @0 :Text;
    labels @1 :List(MetricLabel);
}

struct MetricLabel {
    name @0 :Text;
    value @1 :Text;
}
