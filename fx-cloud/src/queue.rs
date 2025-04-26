use {
    std::{sync::{Arc, RwLock}, collections::{HashMap, HashSet}, thread},
    tracing::error,
    crossbeam::channel::{self, Sender, Receiver},
    crate::{ServiceId, cloud::Engine},
};

const QUEUE_CORE_SIZE_LIMIT: u32 = 100_000;

// more like "queue engine"
#[derive(Clone)]
pub struct Queue {
    inner: Arc<QueueCore>,
}

impl Queue {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            inner: Arc::new(QueueCore::new(engine)),
        }
    }

    pub fn push(&self, queue_id: String, msg: Vec<u8>) {
        self.inner.tx.send(Message::new(queue_id, msg)).unwrap();
    }

    pub fn subscribe(&self, queue_id: String, function_id: ServiceId, rpc_function_name: String) {
        let mut subscriptions = self.inner.subscriptions.write().unwrap();
        if !subscriptions.contains_key(&queue_id) {
            subscriptions.insert(queue_id.clone(), HashSet::new());
        }
        subscriptions.get_mut(&queue_id).unwrap().insert(Subscription::new(function_id, rpc_function_name));
    }

    pub fn run(&self) {
        let core = self.inner.clone();
        thread::spawn(move || {
            loop {
                let msg = core.rx.recv().unwrap();
                let subscriptions = core.subscriptions.read().unwrap();
                let subscriptions = match subscriptions.get(&msg.queue_id) {
                    Some(v) => v,
                    None => continue,
                };
                for subscription in subscriptions {
                    let engine = core.cloud_engine.clone();
                    if let Err(err) = core.cloud_engine.invoke_service_raw(engine, &subscription.service_id, &subscription.rpc_function_name, msg.body.clone()) {
                        error!("failed to invoke queue subscription: {err:?}");
                    }
                }
            }
        });
    }
}

struct QueueCore {
    cloud_engine: Arc<Engine>,

    subscriptions: RwLock<HashMap<String, HashSet<Subscription>>>,

    tx: Sender<Message>,
    rx: Receiver<Message>,
}

impl QueueCore {
    pub fn new(cloud_engine: Arc<Engine>) -> Self {
        let (tx, rx) = channel::bounded(QUEUE_CORE_SIZE_LIMIT as usize);
        Self {
            cloud_engine,
            subscriptions: RwLock::new(HashMap::new()),
            tx,
            rx,
        }
    }
}

#[derive(PartialEq, Eq, Hash)]
struct Subscription {
    service_id: ServiceId,
    rpc_function_name: String,
}

impl Subscription {
    pub fn new(service_id: ServiceId, rpc_function_name: String) -> Self {
        Self {
            service_id,
            rpc_function_name,
        }
    }
}

struct Message {
    queue_id: String,
    body: Vec<u8>,
}

impl Message {
    pub fn new(queue_id: String, body: Vec<u8>) -> Self {
        Self {
            queue_id,
            body,
        }
    }
}
