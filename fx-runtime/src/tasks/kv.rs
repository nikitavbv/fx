use {
    std::collections::HashMap,
    tokio::sync::oneshot,
};

pub(crate) struct KvMessage {
    pub(crate) namespace: String,
    pub(crate) operation: KvOperation,
}

pub(crate) enum KvOperation {
    Set {
        key: Vec<u8>,
        value: Vec<u8>,
        on_done: oneshot::Sender<()>,
    },
    Get {
        key: Vec<u8>,
        result: oneshot::Sender<Option<Vec<u8>>>,
    },
}

pub(crate) fn run_kv_task(kv_rx: flume::Receiver<KvMessage>) {
    let mut kv: HashMap<String, HashMap<Vec<u8>, Vec<u8>>> = HashMap::new();

    while let Ok(msg) = kv_rx.recv() {
        let kv = kv.entry(msg.namespace).or_default();

        match msg.operation {
            KvOperation::Set { key, value, on_done } => {
                kv.insert(key, value);
                on_done.send(()).unwrap();
            },
            KvOperation::Get { key, result } => result.send(kv.get(&key).cloned()).unwrap(),
        }
    }
}
