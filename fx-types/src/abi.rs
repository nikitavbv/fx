use num_enum::TryFromPrimitive;

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
