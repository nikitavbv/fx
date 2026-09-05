use {
    std::task::Poll,
    num_enum::TryFromPrimitive,
    zerocopy::{FromBytes, IntoBytes, Immutable, KnownLayout},
};

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum FuturePollResult {
    Ready = 0,
    Pending = 1,
    NotFound = 2,
}

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum ResourceMoveFromHostResult {
    Ok = 0,
    // bad request:
    FailedToAccessMemory = 1,
    ArgumentOutOfMemoryBounds = 2,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct KvGetResponseFuturePollResult {
    pub tag: u8, // 0 - ready, 1 - pending
    pub _pad: [u8; 7],
    pub kv_get_response_resource_id: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct KvGetResponseSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct KvSetResponseFuturePollResult {
    pub tag: u8, // 0 - ready, 1 - pending
    pub _pad: [u8; 7],
    pub kv_set_response_resource_id: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct KvSetResponseSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct UnitFuturePollResult {
    pub tag: u8, // 0 - ready, 1 - pending
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SqlQueryResultFuturePollResult {
    pub tag: u8, // 0 - ready, 1 - pending
    pub _pad: [u8; 7],
    pub sql_query_result_resource_id: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SqlQueryResultSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SqlBatchResultFuturePollResult {
    pub tag: u8, // 0 - ready, 1 - pending,
    pub _pad: [u8; 7],
    pub sql_batch_result_resource_id: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SqlBatchResultSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct SqlMigrationResultSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct FetchResultFuturePollResult {
    pub tag: u8, // 0 - ready, 1 - pending
    pub _pad: [u8; 7],
    pub fetch_result_resource_id: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct FetchResultSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct HttpBodyPollFrameResult {
    pub tag: u8, // 0 - ready, 1 - pending
    pub _pad: [u8; 7],
    pub http_frame_resource_id: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct HttpFrameSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum HttpFrameSerializeResultCode {
    Ok = 0,
    NotFound = 1,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct AsyncResourcePollResult {
    pub tag: u8, // 0 - ready, 1 - pending
    pub _pad: [u8; 7],
    pub resolved_resource_id: u64,
}

impl<T: Into<u64>> From<Poll<T>> for AsyncResourcePollResult {
    fn from(value: Poll<T>) -> Self {
        match value {
            Poll::Pending => Self {
                tag: 1,
                _pad: Default::default(),
                resolved_resource_id: 0,
            },
            Poll::Ready(v) => Self {
                tag: 0,
                _pad: Default::default(),
                resolved_resource_id: v.into(),
            }
        }
    }
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct AsyncStreamResourcePollResult {
    pub tag: u8, // 0 - stream finished, 1 - next item ready, 2 - pending
    pub _pad: [u8; 7],
    pub resolved_resource_id: u64,
}

impl<T: Into<u64>> From<Poll<Option<T>>> for AsyncStreamResourcePollResult {
    fn from(value: Poll<Option<T>>) -> Self {
        match value {
            Poll::Pending => Self {
                tag: 2,
                _pad: Default::default(),
                resolved_resource_id: 0,
            },
            Poll::Ready(Some(v)) => Self {
                tag: 1,
                _pad: Default::default(),
                resolved_resource_id: v.into(),
            },
            Poll::Ready(None) => Self {
                tag: 0,
                _pad: Default::default(),
                resolved_resource_id: 0,
            }
        }
    }
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct ResourceSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Default)]
pub struct KvSubscriptionStreamPollResult {
    // 0 - stream finished
    // 1 - next item ready
    // 2 - pending
    // 3 - error: runtime shutdown
    // 4 - binding not found
    // 5 - bad request
    // 6 - failed to read request
    pub tag: u8,
    pub _pad: [u8; 7],
    pub resolved_resource_id: u64,
}

// exported by function
#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct FunctionBytesPtrAndLenResult {
    pub ptr: u64,
    pub len: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Default)]
pub struct FunctionHttpBodyFramePollResult {
    pub tag: u8, // 0 - stream end, 1 - ready, 2 - pending, 3 - application error
    pub _pad: [u8; 7],
    pub frame_bytes_resource_id: u64,
    pub frame_bytes_addr: u64,
    pub frame_bytes_len: u64,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Default)]
pub struct FunctionResponsePollResult {
    pub tag: u8, // 1 - pending, 0 - ready
    pub _pad: [u8; 7],
    pub response_bytes_resource_id: u64,
    pub response_bytes_addr: u64,
    pub response_bytes_len: u64,
}

#[derive(TryFromPrimitive)]
#[repr(u64)]
pub enum EnvLenResultCode {
    Ok = 0,
    FailedToReadRequest = 1,
    BadRequest = 2,
    NotFound = 3,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Default)]
pub struct EnvLenResult {
    pub len: u64,
}

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum EnvGetResult {
    Ok = 0,
    FailedToReadRequest = 1,
    BadRequest = 2,
    NotFound = 3,
    FailedToWriteValue = 4,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout, Default)]
pub struct MetricsCounterRegisterResult {
    pub counter_id: u64,
}

#[derive(TryFromPrimitive)]
#[repr(u64)]
pub enum MetricsCounterRegisterResultCode {
    Ok = 0,
    FailedToReadRequest = 1,
    BadRequest = 2,
}

#[derive(TryFromPrimitive)]
#[repr(u64)]
pub enum RandomResultCode {
    Ok = 0,
    FailedToGenerate = 1,
    FailedToReadRequest = 2,
    BadRequest = 3,
}
