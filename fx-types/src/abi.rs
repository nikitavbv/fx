use num_enum::TryFromPrimitive;

#[derive(TryFromPrimitive)]
#[repr(i64)]
pub enum FuturePollResult {
    Ready = 0,
    Pending = 1,
}
