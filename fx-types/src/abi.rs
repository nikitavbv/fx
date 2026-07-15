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
pub struct BlobGetResultSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum BlobGetResultSerializeResultCode {
    Ok = 0,
    NotFound = 1,
}

#[repr(C)]
#[derive(FromBytes, IntoBytes, Immutable, KnownLayout)]
pub struct BlobDeleteResultSerializeResult {
    pub bytes_resource_id: u64,
    pub bytes_length: u64,
}

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum BlobDeleteResultSerializeResultCode {
    Ok = 0,
    NotFound = 1,
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
