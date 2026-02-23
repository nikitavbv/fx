use {
    tokio::time::Duration,
    crate::{
        triggers::cron::CronDatabase,
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

struct CronTask {
    task_id: String,
    function_id: FunctionId,
    schedule: cron::Schedule,
}

pub(crate) fn run_cron_task(
    database: CronDatabase,
    workers_controller: WorkersController,
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
                    match message.unwrap() {
                        CronMessage::ScheduleAdd { function_id, schedule } => {
                            tasks = schedule.into_iter().map(|trigger| CronTask {
                                task_id: trigger.id,
                                function_id: function_id.clone(),
                                schedule: trigger.schedule,
                            }).chain(tasks.into_iter()).collect();
                        },
                    }
                },

                _ = cron_interval.tick() => {
                    // TODO
                }
            }
        }
    }));
}
