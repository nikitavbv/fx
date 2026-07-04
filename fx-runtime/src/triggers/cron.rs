use {
    tracing::error,
    chrono::{DateTime, Utc, NaiveDateTime},
    crate::effects::sql::{SqlDatabase, Query, SqlValue},
};

const DATE_TIME_FORMAT: &str = "%F %T%.f";

pub(crate) struct CronDatabase {
    database: SqlDatabase,
}

impl CronDatabase {
    pub(crate) fn new(database: SqlDatabase) -> Result<Self, ()> {
        database.exec(Query::new("create table if not exists cron_tasks (task_id text primary key, last_run_at datetime)".to_owned()))
            .map_err(|err| {
                error!("failed to create cron_tasks table when creating cron database: {err:?}");
                ()
            })?;

        Ok(Self {
            database,
        })
    }

    pub(crate) fn get_prev_run_time(&self, task_id: &str) -> Result<Option<DateTime<Utc>>, ()> {
        let result = self.database.exec(
            Query::new("select last_run_at from cron_tasks where task_id = ?".to_owned())
                .with_param(SqlValue::Text(task_id.to_owned()))
        ).map_err(|err| {
            error!("get_prev_run_time: sql query failed: {err:?}");
            ()
        })?;

        let result_row = match result.rows.first() {
            Some(v) => v,
            None => return Ok(None),
        };

        let result_column = match result_row.columns.first() {
            Some(v) => v,
            None => {
                error!("get_prev_run_time: expected at least one column to be present in results.");
                return Err(());
            }
        };

        let datetime = match result_column {
            SqlValue::Text(text) => text,
            other => {
                error!("get_prev_run_time: unexpected type for last_run_at: {other:?}");
                return Err(());
            }
        };
        let datetime = match NaiveDateTime::parse_from_str(datetime, DATE_TIME_FORMAT) {
            Ok(v) => v,
            Err(err) => {
                error!("get_prev_run_time failed, failed to parse {datetime:?}: {err:?}");
                return Err(());
            }
        };

        Ok(Some(datetime.and_utc()))
    }

    pub(crate) fn update_run_time(&self, task_id: &str, run_at: DateTime<Utc>) -> Result<(), ()> {
        self.database.exec(
            Query::new("insert or replace into cron_tasks (task_id, last_run_at) values (?, ?)".to_owned())
                .with_param(SqlValue::Text(task_id.to_owned()))
                .with_param(SqlValue::Text(run_at.format(DATE_TIME_FORMAT).to_string()))
        ).map(|_| ()).map_err(|err| {
            error!("failed to update run time: {err:?}");
            ()
        })
    }
}
