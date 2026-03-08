use {
    std::{collections::HashMap, time::Instant},
    tokio::sync::oneshot,
    crate::effects::kv::KvSetRequest,
};

pub(crate) struct KvMessage {
    pub(crate) namespace: String,
    pub(crate) operation: KvOperation,
}

pub(crate) enum KvOperation {
    Set(KvSetRequest, oneshot::Sender<()>),
    Get {
        key: Vec<u8>,
        result: oneshot::Sender<Option<Vec<u8>>>,
    },
}

struct Value {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

struct Kv {
    kv: HashMap<Vec<u8>, Value>,
}

impl Default for Kv {
    fn default() -> Self {
        Self::new()
    }
}

impl Kv {
    fn new() -> Self {
        Self {
            kv: HashMap::new(),
        }
    }

    fn set(&mut self, key: Vec<u8>, value: Vec<u8>) {
        self.kv.insert(key, Value {
            value,
            expires_at: None,
        });
    }

    fn get(&self, current_time: Instant, key: &Vec<u8>) -> Option<&Vec<u8>> {
        let key = self.kv.get(key)?;

        if let Some(expires_at) = key.expires_at {
            if expires_at <= current_time {
                return None;
            }
        }

        return Some(&key.value);
    }
}

pub(crate) fn run_kv_task(kv_rx: flume::Receiver<KvMessage>) {
    let mut kv: HashMap<String, Kv> = HashMap::new();

    while let Ok(msg) = kv_rx.recv() {
        let kv = kv.entry(msg.namespace).or_default();
        let current_time = Instant::now();

        match msg.operation {
            KvOperation::Set(request, result) => {
                kv.set(request.key, request.value);
                result.send(()).unwrap();
            },
            KvOperation::Get { key, result } => result.send(kv.get(current_time, &key).cloned()).unwrap(),
        }
    }
}
