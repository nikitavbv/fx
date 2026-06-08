use {
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
