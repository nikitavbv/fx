use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct Function {
    pub id: String,
}

// events
#[derive(Serialize, Deserialize)]
pub struct FunctionInvokeEvent {
    pub function_id: String,
}
