use {
    std::collections::HashMap,
    serde::Serialize,
    serde_json::Value,
    crate::EventFieldValue,
};

pub fn object_to_event_fields<T: Serialize>(object: T) -> Option<HashMap<String, EventFieldValue>> {
    match value_to_event_field_value(serde_json::to_value(object).ok()?)? {
        EventFieldValue::Object(v) => Some(v.into_iter()
            .map(|(k, v)| (k, *v))
            .collect()),
        _ => None,
    }
}

fn value_to_event_field_value(v: Value) -> Option<EventFieldValue> {
    Some(match v {
        Value::String(v) => EventFieldValue::Text(v),
        Value::Object(v) => EventFieldValue::Object(v.into_iter()
            .filter_map(|(k, v)| value_to_event_field_value(v).map(|v| (k, Box::new(v))))
            .collect()
        ),
        _ => return None
    })
}
