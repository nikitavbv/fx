use {
    std::{collections::HashMap, time::SystemTime},
    tokio::sync::oneshot,
    crate::effects::kv::{KvSetRequest, KvSetError},
};

pub(crate) struct KvMessage {
    pub(crate) namespace: String,
    pub(crate) operation: KvOperation,
}

pub(crate) enum KvOperation {
    Set(KvSetRequest, oneshot::Sender<Result<(), KvSetError>>),
    Get {
        key: Vec<u8>,
        result: oneshot::Sender<Option<Vec<u8>>>,
    },
}

struct Value {
    value: Vec<u8>,
    expires_at: Option<SystemTime>,
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

    fn current_time(&self) -> SystemTime {
        SystemTime::now()
    }

    fn set(&mut self, request: KvSetRequest) -> Result<(), KvSetError> {
        if request.nx {
            if self.get(&request.key).is_some() {
                return Err(KvSetError::AlreadyExists);
            }
        }

        self.kv.insert(request.key, Value {
            value: request.value,
            expires_at: request.px.map(|v| self.current_time() + v),
        });

        Ok(())
    }

    fn get(&self, key: &Vec<u8>) -> Option<&Vec<u8>> {
        let key = self.kv.get(key)?;

        if let Some(expires_at) = key.expires_at {
            if expires_at <= self.current_time() {
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

        match msg.operation {
            KvOperation::Set(request, result) => result.send(kv.set(request)).unwrap(),
            KvOperation::Get { key, result } => result.send(kv.get(&key).cloned()).unwrap(),
        }
    }
}
