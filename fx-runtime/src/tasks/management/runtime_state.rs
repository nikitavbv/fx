use {
    std::{cell::RefCell, rc::Rc},
    crate::function::FunctionId,
};

#[derive(Clone)]
pub struct RuntimeState {
    functions: Rc<RefCell<Vec<FunctionId>>>,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            functions: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn functions(&self) -> Vec<FunctionId> {
        self.functions.borrow().clone()
    }

    pub fn add_function(&self, function_id: FunctionId) {
        let mut functions = self.functions.borrow_mut();
        functions.retain(|f| f != &function_id);
        functions.push(function_id);
    }

    pub fn remove_function(&self, function_id: &FunctionId) {
        let mut functions = self.functions.borrow_mut();
        functions.retain(|f| f != function_id);
    }
}
