use {
    chrono::{DateTime, Utc, NaiveDateTime},
    crate::effects::sql::{SqlDatabase, Query, SqlValue},
};

const DATE_TIME_FORMAT: &str = "%F %T%.f";

pub(crate) struct CronDatabase {
    database: SqlDatabase,
}

impl CronDatabase {
    pub(crate) fn new(database: SqlDatabase) -> Self {
        database.exec(Query::new("create table if not exists cron_tasks (task_id text primary key, last_run_at datetime)".to_owned()))
            .unwrap();

        Self {
            database,
        }
    }

    pub(crate) fn get_prev_run_time(&self, task_id: &str) -> Option<DateTime<Utc>> {
        let result = self.database.exec(
            Query::new("select last_run_at from cron_tasks where task_id = ?".to_owned())
                .with_param(SqlValue::Text(task_id.to_owned()))
        ).unwrap();

        let datetime = match result.rows.get(0)?.columns.get(0).unwrap() {
            SqlValue::Text(text) => text,
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

    pub(crate) fn update_run_time(&self, task_id: &str, run_at: DateTime<Utc>) {
        self.database.exec(
            Query::new("insert or replace into cron_tasks (task_id, last_run_at) values (?, ?)".to_owned())
                .with_param(SqlValue::Text(task_id.to_owned()))
                .with_param(SqlValue::Text(run_at.format(DATE_TIME_FORMAT).to_string()))
        ).unwrap();
    }
}
