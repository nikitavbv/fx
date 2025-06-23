use {
    std::{sync::Arc, time::Duration, str::FromStr},
    tracing::error,
    chrono::{DateTime, Utc, NaiveDateTime},
    tokio::time::sleep,
    cron as cron_utils,
    crate::{sql::{SqlDatabase, Query, Value}, ServiceId, cloud::Engine, error::FxCloudError},
};

const DATE_TIME_FORMAT: &str = "%F %T%.f";

#[derive(Clone)]
pub struct CronTaskDefinition {
    id: String,
    schedule: cron_utils::Schedule,
    function_id: String,
    method_name: String,
}

impl CronTaskDefinition {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            schedule: cron_utils::Schedule::from_str("* * * * * *").unwrap(),
            function_id: "cron".to_owned(),
            method_name: "on_cron".to_owned(),
        }
    }

    pub fn with_schedule(mut self, schedule: String) -> Self {
        self.schedule = cron_utils::Schedule::from_str(schedule.as_str()).unwrap();
        self
    }

    pub fn with_function_id(mut self, function_id: String) -> Self {
        self.function_id = function_id;
        self
    }

    pub fn with_method_name(mut self, method_name: String) -> Self {
        self.method_name = method_name;
        self
    }
}

#[derive(Clone)]
pub struct CronRunner {
    cloud_engine: Arc<Engine>,
    tasks: Vec<CronTaskDefinition>,
    database: CronDatabase,
}

impl CronRunner {
    pub fn new(cloud_engine: Arc<Engine>, database: SqlDatabase) -> Self {
        Self {
            cloud_engine,
            tasks: Vec::new(),
            database: CronDatabase::new(database),
        }
    }

    pub fn with_task(mut self, task: CronTaskDefinition) -> Self {
        self.tasks.push(task);
        self
    }

    pub async fn run(self) {
        let engine = self.cloud_engine;
        let tasks = self.tasks;
        let database = self.database;

        loop {
            for task in &tasks {
                let now = Utc::now();
                match database.get_prev_run_time(&task.id.as_str()) {
                    None => {
                        // first time, let's run
                    },
                    Some(v) => if task.schedule.after(&v).next().unwrap() <= now {
                        // time to run
                    } else {
                        // too early to run again
                        continue;
                    },
                }

                let result = engine.invoke_service::<(), ()>(engine.clone(), &ServiceId::new(task.function_id.clone()), &task.method_name, ()).await;
                match result {
                    Ok(_) => {
                        database.update_run_time(&task.id, now);
                    },
                    Err(err) => {
                        error!("failed to run cron task: {err:?}. Will try again...");
                    }
                }
            }

            sleep(Duration::from_secs(1)).await;
        }
    }
}

#[derive(Clone)]
struct CronDatabase {
    database: Arc<SqlDatabase>,
}

impl CronDatabase {
    fn new(database: SqlDatabase) -> Self {
        database.exec(Query::new("create table if not exists cron_tasks (task_id text primary key, last_run_at datetime)".to_owned()))
            .map_err(|err| FxCloudError::CronError { reason: format!("failed to create state table: {err:?}") })
            .unwrap();

        Self {
            database: Arc::new(database),
        }
    }

    fn get_prev_run_time(&self, task_id: &str) -> Option<DateTime<Utc>> {
        let result = self.database.exec(
            Query::new("select last_run_at from cron_tasks where task_id = ?".to_owned())
                .with_param(Value::Text(task_id.to_owned()))
        ).unwrap();

        let datetime = match result.rows.get(0)?.columns.get(0).unwrap() {
            Value::Text(text) => text,
            other => panic!("unexpected type for last_run_at: {other:?}"),
        };
        let datetime = match NaiveDateTime::parse_from_str(datetime, DATE_TIME_FORMAT) {
            Ok(v) => v,
            Err(err) => {
                panic!("failed to parse {datetime:?}: {err:?}");
            }
        };

        Some(datetime.and_utc())
    }

    fn update_run_time(&self, task_id: &str, run_at: DateTime<Utc>) {
        self.database.exec(
            Query::new("insert or replace into cron_tasks (task_id, last_run_at) values (?, ?)".to_owned())
                .with_param(Value::Text(task_id.to_owned()))
                .with_param(Value::Text(run_at.format(DATE_TIME_FORMAT).to_string()))
        ).unwrap();
    }
}
