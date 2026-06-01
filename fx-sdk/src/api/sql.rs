use {
    fx_types::{
        abi::{SqlQueryResultFuturePollResult, SqlQueryResultSerializeResult},
        capnp,
        abi_sql_capnp,
    },
    crate::{
        sql::{SqlResult, SqlError},
        sys::{fx_sql_query_result_future_poll, fx_sql_query_result_serialize, fx_bytes_move},
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
            other => todo!(),
        }
    }
}
