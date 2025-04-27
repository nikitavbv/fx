use {
    std::fmt::Display,
    sqlx_core::{decode::Decode, types::Type, type_info::TypeInfo},
    super::FxDatabase,
};

#[derive(PartialEq, Clone, Debug)]
pub enum FxDatabaseTypeInfo {
    Integer,
    Text,
}

impl TypeInfo for FxDatabaseTypeInfo {
    fn is_null(&self) -> bool {
        match self {
            Self::Integer
            | Self::Text => false,
        }
    }

    fn name(&self) -> &str {
        unimplemented!()
    }
}

impl Display for FxDatabaseTypeInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}

impl Type<FxDatabase> for String {
    fn type_info() -> FxDatabaseTypeInfo {
        FxDatabaseTypeInfo::Text
    }
}

impl<'r> Decode<'r, FxDatabase> for String {
    fn decode(value: <FxDatabase as sqlx::Database>::ValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        value.text()
    }
}

impl Type<FxDatabase> for u64 {
    fn type_info() -> FxDatabaseTypeInfo {
        FxDatabaseTypeInfo::Integer
    }
}

impl<'r> Decode<'r, FxDatabase> for u64 {
    fn decode(value: <FxDatabase as sqlx::Database>::ValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        value.int64().map(|v| v as u64)
    }
}
