use {
    std::{sync::Arc, collections::{HashMap, HashSet}},
    tracing::{error, info},
    tokio::sync::{RwLock, Mutex, mpsc::{self, Sender, Receiver}},
    serde::{Serialize, Deserialize},
    crate::{ServiceId, cloud::Engine},
};

pub const QUEUE_RPC: &str = "system/rpc";
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

    pub async fn push(&self, queue_id: String, msg: Vec<u8>) {
        self.inner.tx.send(Message::new(queue_id, msg)).await.unwrap();
    }

    pub async fn subscribe(&self, queue_id: String, function_id: ServiceId, rpc_function_name: String) {
        let mut subscriptions = self.inner.subscriptions.write().await;
        if !subscriptions.contains_key(&queue_id) {
            subscriptions.insert(queue_id.clone(), HashSet::new());
        }
        subscriptions.get_mut(&queue_id).unwrap().insert(Subscription::new(function_id, rpc_function_name));
    }

    pub fn run(&self) {
        let core = self.inner.clone();

        let join_handle = tokio::task::spawn_blocking(async move || {
            let mut rx = core.rx.lock().await;

            loop {
                let msg = rx.recv().await.unwrap();
                info!("here {}", msg.queue_id);

                if msg.queue_id == QUEUE_RPC {
                    let engine = core.cloud_engine.clone();
                    let msg: AsyncRpcMessage = rmp_serde::from_slice(&msg.body).unwrap();
                    let result = engine.clone().invoke_service_raw(engine, msg.function_id, msg.rpc_function_name, msg.argument).unwrap().await;
                    if let Err(err) = result {
                        error!("async rpc call failed: {err:?}");
                    }
                }

                let subscriptions = core.subscriptions.read().await;
                let subscriptions = match subscriptions.get(&msg.queue_id) {
                    Some(v) => v,
                    None => continue,
                };
                for subscription in subscriptions {
                    let engine = core.cloud_engine.clone();
                    let body = msg.body.clone();
                    let result = engine.clone().invoke_service_raw(engine, subscription.service_id.clone(), subscription.rpc_function_name.clone(), body).unwrap().await;
                    if let Err(err) = result {
                        error!("failed to invoke queue subscription: {err:?}");
                    }
                }
            }
        });
        tokio::spawn(async { join_handle.await.unwrap().await; });
    }
}

struct QueueCore {
    cloud_engine: Arc<Engine>,

    subscriptions: RwLock<HashMap<String, HashSet<Subscription>>>,

    tx: Sender<Message>,
    rx: Mutex<Receiver<Message>>,
}

impl QueueCore {
    pub fn new(cloud_engine: Arc<Engine>) -> Self {
        let (tx, rx) = mpsc::channel(QUEUE_CORE_SIZE_LIMIT as usize);
        Self {
            cloud_engine,
            subscriptions: RwLock::new(HashMap::new()),
            tx,
            rx: Mutex::new(rx),
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

#[derive(Serialize, Deserialize)]
pub struct AsyncRpcMessage {
    pub function_id: ServiceId,
    pub rpc_function_name: String,
    pub argument: Vec<u8>,
}
