use {
    std::hash::{DefaultHasher, Hasher, Hash},
    super::{SqlMessage, SqlMigrateMessage}
};

#[derive(Clone)]
pub(crate) struct SqlController {
    sql_tx: flume::Sender<SqlMessage>,
    sql_thread_tx: Vec<flume::Sender<SqlMessage>>,
}

impl SqlController {
    pub fn new(sql_tx: flume::Sender<SqlMessage>, sql_thread_tx: Vec<flume::Sender<SqlMessage>>) -> Self {
        Self {
            sql_tx,
            sql_thread_tx,
        }
    }

    pub fn send_message(&self, message: SqlMessage) -> Result<(), flume::SendError<SqlMessage>> {
        self.sql_tx.send(message)
    }

    pub fn send_message_migrate(&self, message: SqlMigrateMessage) -> Result<(), flume::SendError<SqlMessage>> {
        let shard_id = {
            let mut hasher = DefaultHasher::new();
            message.binding.connection_id.hash(&mut hasher);
            hasher.finish()
        } % self.sql_thread_tx.len() as u64;
        self.sql_thread_tx[shard_id as usize].send(SqlMessage::Migrate(message))
    }
}
