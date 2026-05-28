use {
    num_enum::TryFromPrimitive,
    zerocopy::{FromBytes, IntoBytes, Immutable, KnownLayout},
};

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum FuturePollResult {
    Ready = 0,
    Pending = 1,
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
