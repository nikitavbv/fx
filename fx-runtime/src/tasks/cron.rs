use {
    tracing::info,
    tokio::time::Duration,
    chrono::Utc,
    crate::{
        triggers::{cron::CronDatabase, http::FetchRequestHeader},
        tasks::worker::WorkersController,
        definitions::triggers::CronTrigger,
        function::FunctionId,
    },
};

pub(crate) enum CronMessage {
    ScheduleAdd {
        function_id: FunctionId,
        schedule: Vec<CronTrigger>,
    },
}

#[derive(Debug)]
struct CronTask {
    task_id: String,
    function_id: FunctionId,
    schedule: cron::Schedule,

    endpoint: Option<String>,
}

pub(crate) fn run_cron_task(
    mut database: CronDatabase,
    mut workers_controller: WorkersController,
    msg_rx: flume::Receiver<CronMessage>
) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local_set = tokio::task::LocalSet::new();

    tokio_runtime.block_on(local_set.run_until(async {
        let mut cron_interval = tokio::time::interval(Duration::from_secs(1));

        let mut tasks = Vec::<CronTask>::new();

        loop {
            tokio::select! {
                message = msg_rx.recv_async() => {
                    let message = match message {
                        Ok(v) => v,
                        Err(flume::RecvError::Disconnected) => {
                            info!("stopping cron task, because channel handle is dropped");
                            break;
                        },
                    };

                    match message {
                        CronMessage::ScheduleAdd { function_id, schedule } => {
                            tasks = schedule.into_iter().map(|trigger| CronTask {
                                task_id: trigger.id,
                                function_id: function_id.clone(),
                                schedule: trigger.schedule,
                                endpoint: trigger.endpoint,
                            }).chain(tasks.into_iter()).collect();
                        },
                    }
                },

                _ = cron_interval.tick() => {
                    run_tasks(&mut database, &mut workers_controller, &tasks).await;
                }
            }
        }
    }));
}

async fn run_tasks(database: &mut CronDatabase, workers_controller: &mut WorkersController, tasks: &Vec<CronTask>) {
    for task in tasks {
        if let Some(prev_run_time) = database.get_prev_run_time(&task.task_id) {
            if task.schedule.after(&prev_run_time).next().unwrap() > Utc::now() {
                continue;
            }
        };

        workers_controller.function_invoke(task.function_id.clone(), FetchRequestHeader::from_http_parts({
            http::Request::builder()
                .method(http::Method::GET)
                .uri(task.endpoint.as_ref().map(|v| v.as_str()).unwrap_or("/_fx/cron"))
                .body(())
                .unwrap()
                .into_parts()
                .0
        })).await.unwrap();

        database.update_run_time(&task.task_id, Utc::now());
    }
}
