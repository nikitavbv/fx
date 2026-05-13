use {
    std::{cell::RefCell, collections::{HashMap, HashSet}, rc::Rc},
    chrono::{DateTime, Utc},
    crate::function::FunctionId,
};

#[derive(Clone, Debug)]
pub struct CronTaskInfo {
    pub name: String,
    pub function_id: FunctionId,
    pub schedule: String,
}

#[derive(Clone)]
pub struct RuntimeState {
    functions: Rc<RefCell<Vec<FunctionId>>>,
    cron_tasks: Rc<RefCell<Vec<CronTaskInfo>>>,
    cron_last_runs: Rc<RefCell<HashMap<(FunctionId, String), DateTime<Utc>>>>,
    cron_running: Rc<RefCell<HashSet<(FunctionId, String)>>>,
}

impl RuntimeState {
    pub fn new() -> Self {
        Self {
            functions: Rc::new(RefCell::new(Vec::new())),
            cron_tasks: Rc::new(RefCell::new(Vec::new())),
            cron_last_runs: Rc::new(RefCell::new(HashMap::new())),
            cron_running: Rc::new(RefCell::new(HashSet::new())),
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
        let mut cron_tasks = self.cron_tasks.borrow_mut();
        cron_tasks.retain(|t| &t.function_id != function_id);
    }

    pub fn set_cron_tasks(&self, function_id: FunctionId, tasks: Vec<CronTaskInfo>) {
        let mut cron_tasks = self.cron_tasks.borrow_mut();
        cron_tasks.retain(|t| t.function_id != function_id);
        cron_tasks.extend(tasks);
    }

    pub fn cron_tasks(&self) -> Vec<CronTaskInfo> {
        self.cron_tasks.borrow().clone()
    }

    pub fn record_cron_run(&self, name: String, function_id: FunctionId, run_at: DateTime<Utc>) {
        self.cron_last_runs.borrow_mut().insert((function_id.clone(), name.clone()), run_at);
        self.cron_running.borrow_mut().remove(&(function_id, name));
    }

    pub fn cron_last_run(&self, name: &str, function_id: &FunctionId) -> Option<DateTime<Utc>> {
        self.cron_last_runs.borrow().get(&(function_id.clone(), name.to_owned())).copied()
    }

    pub fn mark_cron_running(&self, name: String, function_id: FunctionId) {
        self.cron_running.borrow_mut().insert((function_id, name));
    }

    pub fn is_cron_running(&self, name: &str, function_id: &FunctionId) -> bool {
        self.cron_running.borrow().contains(&(function_id.clone(), name.to_owned()))
    }
}
