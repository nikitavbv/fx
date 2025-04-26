use {
    std::{sync::{Arc, RwLock}, collections::{HashMap, HashSet}, thread},
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

    pub fn subscribe(&self, queue_id: String, function_id: ServiceId) {
        let mut subscriptions = self.inner.subscriptions.write().unwrap();
        if !subscriptions.contains_key(&queue_id) {
            subscriptions.insert(queue_id.clone(), HashSet::new());
        }
        subscriptions.get_mut(&queue_id).unwrap().insert(function_id);
    }

    pub fn run(&self) {
        let core = self.inner.clone();
        thread::spawn(move || {
            loop {
                let msg = core.rx.recv();
                // TODO: handle this
            }
        });
    }
}

struct QueueCore {
    cloud_engine: Arc<Engine>,

    subscriptions: RwLock<HashMap<String, HashSet<ServiceId>>>,

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
