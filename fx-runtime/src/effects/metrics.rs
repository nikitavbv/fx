use {
    std::{collections::HashMap, sync::RwLock},
    crate::function::FunctionId,
};

#[derive(Debug, Eq, PartialEq, Hash, Clone)]
pub(crate) struct MetricKey {
    pub(crate) name: String,
    pub(crate) labels: Vec<(String, String)>,
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub(crate) struct MetricId {
    id: u64,
}

impl MetricId {
    pub fn new(id: u64) -> Self {
        Self { id }
    }

    pub fn from_abi(id: u64) -> Self {
        Self::new(id)
    }

    pub fn into_abi(&self) -> u64 {
        self.id
    }
}

pub(crate) struct FunctionMetricsState {
    metrics_series: Vec<MetricKey>,
    metric_key_to_ids: HashMap<MetricKey, MetricId>,
    metrics_counter_delta: HashMap<MetricId, u64>,
}

impl FunctionMetricsState {
    pub fn new() -> Self {
        Self {
            metrics_series: Vec::new(),
            metric_key_to_ids: HashMap::new(),
            metrics_counter_delta: HashMap::new(),
        }
    }

    pub(crate) fn counter_register(&mut self, key: MetricKey) -> MetricId {
        *self.metric_key_to_ids.entry(key.clone()).or_insert_with(|| {
            let index = self.metrics_series.len() as u64;
            self.metrics_series.push(key);
            MetricId::new(index)
        })
    }

    pub(crate) fn counter_increment(&mut self, key: MetricId, delta: u64) {
        *self.metrics_counter_delta.entry(key).or_insert(0) += delta;
    }

    pub(crate) fn flush_delta(&mut self) -> FunctionMetricsDelta {
        let counters_delta = std::mem::replace(&mut self.metrics_counter_delta, HashMap::new())
            .into_iter()
            .map(|(metric_id, delta)| (self.metrics_series.get(metric_id.id as usize).unwrap().clone(), delta))
            .collect();

        FunctionMetricsDelta {
            counters_delta,
        }
    }
}

pub(crate) struct MetricsRegistry {
    function_metrics: RwLock<HashMap<FunctionId, FunctionMetricsDelta>>,
}

impl MetricsRegistry {
    pub fn new() -> Self {
        Self {
            function_metrics: RwLock::new(HashMap::new()),
        }
    }

    pub fn update(&self, function_metrics: HashMap<FunctionId, FunctionMetricsDelta>) {
        let mut all_metrics = self.function_metrics.write().unwrap();

        for (function_id, metrics) in function_metrics {
            all_metrics.entry(function_id)
                .or_insert_with(|| FunctionMetricsDelta::empty())
                .append(metrics);
        }
    }

    pub fn encode(&self) -> String {
        let mut result = String::new();
        let all_metrics = self.function_metrics.read().unwrap();

        for (function_id, function_metrics) in all_metrics.iter() {
            let function_id = sanitize_metric_name(function_id.as_str());

            for (counter_key, counter_value) in function_metrics.counters_delta.iter() {
                let metric_name = {
                    let mut metric_name = String::new();
                    metric_name.push_str("function_");
                    metric_name.push_str(sanitize_metric_name(function_id.as_str()).as_str());
                    metric_name.push('_');
                    metric_name.push_str(sanitize_metric_name(counter_key.name.as_str()).as_str());
                    metric_name
                };

                result.push_str("# TYPE ");
                result.push_str(metric_name.as_str());
                result.push_str(" counter\n");
                result.push_str(metric_name.as_str());

                if !counter_key.labels.is_empty() {
                    result.push('{');

                    for (index, (label_key, label_value)) in counter_key.labels.iter().enumerate() {
                        if index > 0 {
                            result.push(',');
                        }
                        result.push_str(sanitize_label_name(label_key.as_str()).as_str());
                        result.push_str("=\"");
                        result.push_str(escape_label_value(label_value.as_str()).as_str());
                        result.push('"');
                    }

                    result.push('}')
                }

                result.push(' ');
                result.push_str(counter_value.to_string().as_str());
                result.push('\n');
            }
        }

        result
    }
}

fn sanitize_metric_name(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphanumeric() || c == '_' || c == ':' {
                c
            } else if i == 0 && c.is_ascii_digit() {
                '_'
            } else {
                '_'
            }
        })
        .collect()
}

fn sanitize_label_name(s: &str) -> String {
    s.chars()
        .enumerate()
        .map(|(i, c)| {
            if c.is_ascii_alphabetic() || c == '_' || (i > 0 && c.is_ascii_digit()) { c } else { '_' }
        })
        .collect()
}

fn escape_label_value(s: &str) -> String {
    s.replace('\\', "\\\\")
     .replace('"', "\\\"")
     .replace('\n', "\\n")
}

#[derive(Debug)]
pub(crate) struct FunctionMetricsDelta {
    counters_delta: HashMap<MetricKey, u64>,
}

impl FunctionMetricsDelta {
    pub fn empty() -> Self {
        Self {
            counters_delta: HashMap::new(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.counters_delta.is_empty()
    }

    pub(crate) fn append(&mut self, other: FunctionMetricsDelta) {
        for (metric_key, delta) in other.counters_delta {
            *self.counters_delta.entry(metric_key).or_insert(0) += delta;
        }
    }
}
