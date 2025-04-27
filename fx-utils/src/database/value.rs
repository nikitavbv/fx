use {
    std::borrow::Cow,
    sqlx_core::{error::BoxDynError, value::ValueRef},
    super::{FxDatabase, FxDatabaseTypeInfo},
};

#[derive(Debug)]
pub enum FxDatabaseValue {
    Integer(i64),
    Text(String),
}

pub struct FxDatabaseValueRef<'r>(pub(crate) &'r FxDatabaseValue);

impl FxDatabaseValue {
    fn text(&self) -> Result<String, BoxDynError> {
        match self {
            Self::Text(text) => Ok(text.clone()),
            _ => panic!("wrong type"),
        }
    }

    fn int64(&self) -> Result<i64, BoxDynError> {
        match self {
            Self::Integer(v) => Ok(*v),
            _ => panic!("wrong type"),
        }
    }
}

impl<'r> FxDatabaseValueRef<'r> {
    pub(crate) fn text(&self) -> Result<String, BoxDynError> {
        self.0.text()
    }

    pub(crate) fn int64(&self) -> Result<i64, BoxDynError> {
        self.0.int64()
    }
}

impl<'r> ValueRef<'r> for FxDatabaseValueRef<'r> {
    type Database = FxDatabase;

    fn to_owned(&self) -> FxDatabaseValue {
        unimplemented!()
    }

    fn type_info(&self) -> Cow<'_, FxDatabaseTypeInfo> {
        Cow::Owned(match self.0 {
            FxDatabaseValue::Integer(_) => FxDatabaseTypeInfo::Integer,
            FxDatabaseValue::Text(_) => FxDatabaseTypeInfo::Text,
        })
    }

    fn is_null(&self) -> bool {
        match self.0 {
            FxDatabaseValue::Integer(_)
            | FxDatabaseValue::Text(_) => false,
        }
    }
}
