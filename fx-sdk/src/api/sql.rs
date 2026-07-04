use {
    fx_types::{
        abi::{
            SqlQueryResultFuturePollResult,
            SqlQueryResultSerializeResult,
            SqlBatchResultFuturePollResult,
            SqlBatchResultSerializeResult,
            SqlMigrationResultSerializeResult,
            AsyncResourcePollResult,
        },
        capnp,
        abi_sql_capnp,
    },
    crate::{
        sql::{SqlResult, SqlError, SqlBatchError},
        sys::{
            fx_sql_query_result_future_poll,
            fx_sql_query_result_serialize,
            fx_bytes_move,
            fx_sql_batch_result_future_poll,
            fx_sql_batch_result_serialize,
            fx_migration_result_future_poll,
            fx_migration_result_serialize,
        },
        utils::migrations::SqlMigrationError,
    },
};

pub(crate) struct SqlQueryResultFuture(u64);

impl SqlQueryResultFuture {
    pub fn new(resource_id: u64) -> Self {
        Self(resource_id)
    }
}

impl Future for SqlQueryResultFuture {
    type Output = Result<SqlResult, SqlError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<SqlQueryResultFuturePollResult>::zeroed();
        assert!(unsafe { fx_sql_query_result_future_poll(self.0, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };
        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<SqlQueryResultSerializeResult>::zeroed();
                assert!(unsafe { fx_sql_query_result_serialize(result.sql_query_result_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let request = resource_reader.get_root::<fx_types::abi_sql_capnp::sql_exec_result::Reader>().unwrap();

                match request.get_result().which().unwrap() {
                    abi_sql_capnp::sql_exec_result::result::Which::Rows(v) => Ok(SqlResult::from(v.unwrap())),
                    abi_sql_capnp::sql_exec_result::result::Which::Error(err) => Err(match err.unwrap().get_error().which().unwrap() {
                        abi_sql_capnp::sql_exec_error::error::Which::BindingNotFound(_) => SqlError::BindingNotFound,
                        abi_sql_capnp::sql_exec_error::error::Which::DatabaseBusy(_) => SqlError::DatabaseBusy,
                        abi_sql_capnp::sql_exec_error::error::Which::RuntimeShutdown(_) => SqlError::RuntimeShutdown,
                        abi_sql_capnp::sql_exec_error::error::Which::StatementError(reason) => SqlError::StatementError(reason.unwrap().to_string().unwrap()),
                    }),
                }
            }),
            _other => std::task::Poll::Ready(Err(SqlError::InternalSdkError)),
        }
    }
}

pub(crate) struct SqlBatchResultFuture(u64);

impl SqlBatchResultFuture {
    pub fn new(resource_id: u64) -> Self {
        Self(resource_id)
    }
}

impl Future for SqlBatchResultFuture {
    type Output = Result<(), SqlBatchError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<SqlBatchResultFuturePollResult>::zeroed();
        assert!(unsafe { fx_sql_batch_result_future_poll(self.0, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };
        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<SqlBatchResultSerializeResult>::zeroed();
                assert!(unsafe { fx_sql_batch_result_serialize(result.sql_batch_result_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let resource_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let request = resource_reader.get_root::<fx_types::abi_sql_capnp::sql_batch_result::Reader>().unwrap();

                match request.get_result().which().unwrap() {
                    abi_sql_capnp::sql_batch_result::result::Which::Ok(_) => Ok(()),
                    abi_sql_capnp::sql_batch_result::result::Which::Error(err) => Err(match err.unwrap().get_error().which().unwrap() {
                        abi_sql_capnp::sql_batch_error::error::Which::BindingNotFound(_) => SqlBatchError::BindingNotFound,
                        abi_sql_capnp::sql_batch_error::error::Which::DatabaseBusy(_) => SqlBatchError::DatabaseBusy,
                        abi_sql_capnp::sql_batch_error::error::Which::StatementFailed(err) => SqlBatchError::StatementFailed { reason: err.unwrap().to_string().unwrap() },
                        abi_sql_capnp::sql_batch_error::error::Which::RuntimeShutdown(_) => SqlBatchError::RuntimeShutdown,
                    }),
                }
            }),
            _other => std::task::Poll::Ready(Err(SqlBatchError::InternalSdkError)),
        }
    }
}

pub(crate) struct SqlMigrateResultFuture(u64);

impl SqlMigrateResultFuture {
    pub fn new(resource_id: u64) -> Self {
        Self(resource_id)
    }
}

impl Future for SqlMigrateResultFuture {
    type Output = Result<(), SqlMigrationError>;

    fn poll(self: std::pin::Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        let mut result = std::mem::MaybeUninit::<AsyncResourcePollResult>::zeroed();
        assert!(unsafe { fx_migration_result_future_poll(self.0, result.as_mut_ptr() as u64) } == 0);

        let result = unsafe { result.assume_init() };
        match result.tag {
            1 => std::task::Poll::Pending,
            0 => std::task::Poll::Ready({
                let mut serialization_result = std::mem::MaybeUninit::<SqlMigrationResultSerializeResult>::zeroed();
                assert!(unsafe { fx_migration_result_serialize(result.resolved_resource_id, serialization_result.as_mut_ptr() as u64) } == 0);

                let result = unsafe { serialization_result.assume_init() };
                let mut result_vec = vec![0; result.bytes_length as usize];
                unsafe { fx_bytes_move(result.bytes_resource_id, result_vec.as_mut_ptr() as u64) };

                let result_reader = capnp::serialize::read_message_from_flat_slice(&mut result_vec.as_slice(), capnp::message::ReaderOptions::default()).unwrap();
                let result = result_reader.get_root::<abi_sql_capnp::sql_migrate_result::Reader>().unwrap();

                match result.get_result().which().unwrap() {
                    abi_sql_capnp::sql_migrate_result::result::Which::Ok(_) => Ok(()),
                    abi_sql_capnp::sql_migrate_result::result::Which::Error(err) => Err(match err.unwrap().get_error().which().unwrap() {
                        abi_sql_capnp::sql_migrate_error::error::Which::BindingNotFound(_) => SqlMigrationError::BindingNotFound,
                        abi_sql_capnp::sql_migrate_error::error::Which::DatabaseBusy(_) => SqlMigrationError::DatabaseBusy,
                        abi_sql_capnp::sql_migrate_error::error::Which::ExecutionError(message) => SqlMigrationError::MigrationExecutionError {
                            message: message.unwrap().to_string().unwrap(),
                        },
                        abi_sql_capnp::sql_migrate_error::error::Which::SqlError(message) => SqlMigrationError::SqlError {
                            message: message.unwrap().to_string().unwrap(),
                        },
                        abi_sql_capnp::sql_migrate_error::error::Which::RuntimeShutdown(_) => SqlMigrationError::RuntimeShutdown,
                    }),
                }
            }),
            _other => std::task::Poll::Ready(Err(SqlMigrationError::InternalSdkError)),
        }
    }
}
