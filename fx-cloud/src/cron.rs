use {
    std::{thread::{spawn, sleep}, sync::Arc, time::Duration, str::FromStr},
    tracing::error,
    chrono::{DateTime, Utc},
    cron as cron_utils,
    fx_core::CronRequest,
    crate::{sql::{SqlDatabase, Query, Value}, ServiceId, cloud::Engine, error::FxCloudError},
};

#[derive(Clone)]
pub struct CronRunner {
    cloud_engine: Arc<Engine>,
    cron: Arc<CronCore>,
}

impl CronRunner {
    pub fn new(cloud_engine: Arc<Engine>, database: SqlDatabase) -> Result<Self, FxCloudError> {
        Ok(Self {
            cloud_engine,
            cron: Arc::new(CronCore::new(database)?),
        })
    }

    pub fn schedule(&self, cron_expression: impl Into<String>, function_id: ServiceId, rpc_function_name: String) -> Result<(), FxCloudError> {
        self.cron.schedule(cron_expression.into(), function_id.into(), rpc_function_name)?;
        Ok(())
    }

    pub fn run(&self) {
        let cloud_engine = self.cloud_engine.clone();
        let cron = self.cron.clone();

        let join_handle = tokio::task::spawn_blocking(async move || {
            loop {
                let tasks_to_run = cron.get_tasks_to_run();

                for task in tasks_to_run {
                    cloud_engine.invoke_service_async(ServiceId::new(task.function_id), task.rpc_function_name, CronRequest {}).await;
                    let next = cron_utils::Schedule::from_str(&task.cron_expression).unwrap().after(&Utc::now()).next().unwrap();
                    if let Err(err) = cron.update_task_next_run_at(task.id, next) {
                        error!("failed to update task next run time: {err:?}");
                    }
                }

                sleep(Duration::from_secs(1));
            }
        });
        tokio::spawn(async { join_handle.await.unwrap().await; });
    }
}

#[derive(Debug)]
struct Task {
    id: i64,
    cron_expression: String,
    function_id: String,
    rpc_function_name: String,
}

struct CronCore {
    database: SqlDatabase,
}

impl CronCore {
    pub fn new(database: SqlDatabase) -> Result<Self, FxCloudError> {
        database.exec(Query::new("create table if not exists cron_tasks (task_id integer primary key, cron_expression text not null, function_id text not null, rpc_function_name text not null, next_run_at datetime)".to_owned()))
            .map_err(|err| FxCloudError::CronError { reason: format!("failed to create state table: {err:?}") })?;
        Ok(Self {
            database,
        })
    }

    fn schedule(&self, cron_expression: String, function_id: String, rpc_function_name: String) -> Result<(), FxCloudError> {
        self.database.exec(
            Query::new("insert into cron_tasks (cron_expression, function_id, rpc_function_name, next_run_at) values (?, ?, ?, ?)".to_owned())
                .with_param(Value::Text(cron_expression))
                .with_param(Value::Text(function_id))
                .with_param(Value::Text(rpc_function_name))
                .with_param(Value::Null)
        ).map_err(|err| FxCloudError::CronError { reason: format!("failed to schedule cron task: {err:?}") })?;
        Ok(())
    }

    fn get_tasks_to_run(&self) -> Vec<Task> {
        let res = self.database.exec(Query::new("select task_id, cron_expression, function_id, rpc_function_name from cron_tasks where next_run_at is null or next_run_at <= CURRENT_TIMESTAMP".to_owned()));
        let res = match res {
            Ok(v) => v,
            Err(err) => {
                error!("failed to load tasks to run from database: {err:?}");
                return Vec::new();
            }
        };

        res.rows
            .into_iter()
            .map(|v| Task {
                id: v.columns.get(0).unwrap().try_into().unwrap(),
                cron_expression: v.columns.get(1).unwrap().try_into().unwrap(),
                function_id: v.columns.get(2).unwrap().try_into().unwrap(),
                rpc_function_name: v.columns.get(3).unwrap().try_into().unwrap(),
            })
            .collect()
    }

    fn update_task_next_run_at(&self, task_id: i64, run_at: DateTime<Utc>) -> Result<(), FxCloudError> {
        self.database.exec(
            Query::new("update cron_tasks set next_run_at = ? where task_id = ?".to_owned())
                .with_param(Value::Text(run_at.format("%F %T%.f").to_string()))
                .with_param(Value::Integer(task_id))
        ).map_err(|err| FxCloudError::CronError { reason: format!("failed to update next run time: {err:?}") })?;
        Ok(())
    }
}
