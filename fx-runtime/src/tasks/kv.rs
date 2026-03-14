use {
    std::{collections::HashMap, time::SystemTime},
    tokio::sync::oneshot,
    crate::effects::kv::{KvSetRequest, KvSetError, KvDelexRequest, KvPublishRequest},
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
    Delex(KvDelexRequest, oneshot::Sender<()>),
    Subscribe {
        channel: Vec<u8>,
        result: oneshot::Sender<flume::Receiver<Vec<u8>>>,
    },
    Publish(KvPublishRequest, oneshot::Sender<()>),
}

struct Value {
    value: Vec<u8>,
    expires_at: Option<SystemTime>,
}

struct Channel {
    subscribers: Vec<flume::Sender<Vec<u8>>>,
}

impl Channel {
    pub fn new() -> Self {
        Self {
            subscribers: Vec::new(),
        }
    }

    fn subscribe(&mut self) -> flume::Receiver<Vec<u8>> {
        let (tx, rx) = flume::unbounded();
        self.subscribers.push(tx);
        rx
    }

    fn publish(&self, data: Vec<u8>) {
        for subscriber in &self.subscribers {
            subscriber.send(data.clone()).unwrap();
        }
    }
}

struct Kv {
    kv: HashMap<Vec<u8>, Value>,
    channels: HashMap<Vec<u8>, Channel>,
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
            channels: HashMap::new(),
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

    fn delex(&mut self, request: KvDelexRequest) {
        let key = match self.kv.get(&request.key) {
            Some(v) => v,
            None => return,
        };

        if key.value == request.ifeq {
            self.kv.remove(&request.key);
        }
    }

    fn subscribe(&mut self, channel: Vec<u8>) -> flume::Receiver<Vec<u8>> {
        self.channels.entry(channel).or_insert(Channel::new()).subscribe()
    }

    fn publish(&self, channel: &Vec<u8>, data: Vec<u8>) {
        if let Some(channel) = self.channels.get(channel) {
            channel.publish(data);
        }
    }
}

pub(crate) fn run_kv_task(kv_rx: flume::Receiver<KvMessage>) {
    let mut kv: HashMap<String, Kv> = HashMap::new();

    while let Ok(msg) = kv_rx.recv() {
        let kv = kv.entry(msg.namespace).or_default();

        match msg.operation {
            KvOperation::Set(request, result) => result.send(kv.set(request)).unwrap(),
            KvOperation::Get { key, result } => result.send(kv.get(&key).cloned()).unwrap(),
            KvOperation::Delex(request, result) => {
                kv.delex(request);
                result.send(()).unwrap();
            },
            KvOperation::Subscribe { channel, result } => result.send(kv.subscribe(channel)).unwrap(),
            KvOperation::Publish(request, result) => {
                kv.publish(&request.channel, request.data);
                result.send(()).unwrap();
            },
        }
    }
}
