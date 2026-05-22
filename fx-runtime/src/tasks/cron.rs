use {
    tracing::info,
    tokio::time::{Duration, Instant},
    chrono::{DateTime, Utc, TimeDelta},
    crate::{
        triggers::{cron::CronDatabase, http::FetchRequestHeader},
        tasks::worker::WorkersController,
        definitions::triggers::CronTrigger,
        function::FunctionId,
    },
};

const MINIMUM_CRON_FREQUENCY_MS: u64 = 1000;

#[derive(Clone, Debug)]
pub enum CronTaskEvent {
    Start {
        name: String,
        function_id: FunctionId,
    },
    Run {
        name: String,
        function_id: FunctionId,
        run_at: DateTime<Utc>,
        delay: Option<TimeDelta>, // delay from expected run time. None if runs for first time.
        iteration_delay: Duration, // delay introduced by other tasks running before this task in single cron iteration.
    },
}

pub(crate) enum CronMessage {
    ScheduleAdd {
        function_id: FunctionId,
        schedule: Vec<CronTrigger>,
    },
}

#[derive(Debug)]
struct CronTask {
    name: String,
    task_id: String,
    function_id: FunctionId,
    schedule: cron::Schedule,
    endpoint: Option<String>,
}

pub(crate) fn run_cron_task(
    mut database: CronDatabase,
    mut workers_controller: WorkersController,
    msg_rx: flume::Receiver<CronMessage>,
    cron_events: flume::Sender<CronTaskEvent>,
) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local_set = tokio::task::LocalSet::new();

    tokio_runtime.block_on(local_set.run_until(async {
        let mut tasks = Vec::<CronTask>::new();
        let sleep = tokio::time::sleep(Duration::from_millis(0));
        tokio::pin!(sleep);

        loop {
            tokio::select! {
                _ = &mut sleep => {
                    let next_time = run_tasks(&mut database, &mut workers_controller, &cron_events, &tasks).await;
                    let dur = if let Some(next_time) = next_time {
                        Duration::from_millis((next_time - Utc::now()).num_milliseconds().max(0).min(MINIMUM_CRON_FREQUENCY_MS as i64) as u64)
                    } else {
                        Duration::from_millis(MINIMUM_CRON_FREQUENCY_MS)
                    };
                    sleep.as_mut().reset(Instant::now() + dur);
                }

                message = msg_rx.recv_async() => {
                    let message = match message {
                        Ok(v) => v,
                        Err(flume::RecvError::Disconnected) => {
                            info!("stopping cron task, because channel handle is dropped");
                            break;
                        }
                    };

                    match message {
                        CronMessage::ScheduleAdd { function_id, schedule } => {
                            tasks = schedule.into_iter().map(|trigger| CronTask {
                                name: trigger.name,
                                task_id: trigger.id,
                                function_id: function_id.clone(),
                                schedule: trigger.schedule,
                                endpoint: trigger.endpoint,
                            }).chain(tasks.into_iter()).collect();
                        },
                    }

                    sleep.as_mut().reset(Instant::now());
                }
            }
        }
    }));
}

async fn run_tasks(database: &mut CronDatabase, workers_controller: &mut WorkersController, cron_events: &flume::Sender<CronTaskEvent>, tasks: &Vec<CronTask>) -> Option<DateTime<Utc>> {
    let iteration_start_time = Instant::now();
    let mut next_run: Option<DateTime<Utc>> = None;

    for task in tasks {
        let now = Utc::now();
        let delay = if let Some(prev_run_time) = database.get_prev_run_time(&task.task_id) {
            let next_scheduled_run = task.schedule.after(&prev_run_time).next().unwrap();

            if next_scheduled_run > now {
                continue;
            }

            Some(now - next_scheduled_run)
        } else {
            None
        };
        let task_next_run = task.schedule.after(&now).next().unwrap();
        next_run = Some(next_run.map(|v| v.min(task_next_run)).unwrap_or(task_next_run));

        cron_events.send_async(CronTaskEvent::Start {
            name: task.name.clone(),
            function_id: task.function_id.clone(),
        }).await.unwrap();

        let iteration_delay = Instant::now() - iteration_start_time;
        let result = workers_controller.function_invoke(task.function_id.clone(), FetchRequestHeader::from_http_parts({
            http::Request::builder()
                .method(http::Method::GET)
                .uri(task.endpoint.as_deref().unwrap_or("/_fx/cron"))
                .body(())
                .unwrap()
                .into_parts()
                .0
        })).await;

        let run_at = Utc::now();
        database.update_run_time(&task.task_id, run_at);

        cron_events.send_async(CronTaskEvent::Run {
            name: task.name.clone(),
            function_id: task.function_id.clone(),
            run_at,
            delay,
            iteration_delay,
        }).await.unwrap();

        result.unwrap();
    }

    next_run
}
