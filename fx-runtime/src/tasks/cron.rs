use {
    std::{rc::Rc, cell::RefCell, collections::HashSet},
    tracing::{info, error},
    tokio::time::{Duration, Instant},
    chrono::{DateTime, Utc, TimeDelta},
    futures::{stream::FuturesUnordered, StreamExt, future::LocalBoxFuture, FutureExt},
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

#[derive(Debug, Clone)]
struct CronTask {
    name: String,
    task_id: String,
    function_id: FunctionId,
    schedule: cron::Schedule,
    endpoint: Option<String>,
    timeout: Option<Duration>,
}

pub(crate) fn run_cron_task(
    database: CronDatabase,
    workers_controller: WorkersController,
    msg_rx: flume::Receiver<CronMessage>,
    cron_events: flume::Sender<CronTaskEvent>,
) {
    let tokio_runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build();
    let tokio_runtime = match tokio_runtime {
        Ok(v) => v,
        Err(err) => {
            error!("failed to create tokio runtime: {err:?}. Stopping cron task");
            return;
        }
    };
    let local_set = tokio::task::LocalSet::new();

    tokio_runtime.block_on(local_set.run_until(async {
        let database = Rc::new(database);
        let workers_controller = Rc::new(workers_controller);

        let mut tasks = Vec::<CronTask>::new();
        let sleep = tokio::time::sleep(Duration::from_millis(0));
        tokio::pin!(sleep);

        let running_tasks = Rc::new(RefCell::new(HashSet::new()));
        let mut task_futures = FuturesUnordered::new();

        loop {
            tokio::select! {
                _ = &mut sleep => {
                    let next_time = run_tasks(database.clone(), workers_controller.clone(), &cron_events, running_tasks.clone(), &mut task_futures, tasks.clone()).await;
                    let dur = if let Some(next_time) = next_time {
                        Duration::from_millis((next_time - Utc::now()).num_milliseconds().max(0).min(MINIMUM_CRON_FREQUENCY_MS as i64) as u64)
                    } else {
                        Duration::from_millis(MINIMUM_CRON_FREQUENCY_MS)
                    };
                    sleep.as_mut().reset(Instant::now() + dur);
                }

                Some(res) = task_futures.next(), if !task_futures.is_empty() => {
                    let new_deadline = Instant::now() + (res - Utc::now()).to_std().unwrap_or(Duration::ZERO);
                    if new_deadline < sleep.deadline() {
                        sleep.as_mut().reset(new_deadline);
                    }
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
                                timeout: trigger.timeout,
                            }).chain(tasks.into_iter()).collect();
                        },
                    }

                    sleep.as_mut().reset(Instant::now());
                }
            }
        }
    }));
}

async fn run_tasks<'a>(database: Rc<CronDatabase>, workers_controller: Rc<WorkersController>, cron_events: &'a flume::Sender<CronTaskEvent>, running_tasks: Rc<RefCell<HashSet<String>>>, task_futures: &mut FuturesUnordered<LocalBoxFuture<'a, DateTime<Utc>>>, tasks: Vec<CronTask>) -> Option<DateTime<Utc>> {
    let iteration_start_time = Instant::now();

    let mut next_run = None;

    for task in tasks {
        if running_tasks.borrow().contains(&task.task_id) {
            continue;
        }

        let now = Utc::now();

        let prev_run_time = match database.get_prev_run_time(&task.task_id) {
            Ok(v) => v,
            Err(_) => {
                error!("failed to get previous run time for task. Skipping");
                continue;
            }
        };

        let delay = if let Some(prev_run_time) = prev_run_time {
            let next_scheduled_run = match task.schedule.after(&prev_run_time).next() {
                Some(v) => v,
                None => {
                    // TODO: report this error to management thread or logs.
                    error!("could not calculate next cron task run date according to schedule, skipping task from running");
                    continue;
                }
            };

            if next_scheduled_run > now {
                next_run = next_run.map(|v| if v < next_scheduled_run { v } else { next_scheduled_run });
                continue;
            }

            Some(now - next_scheduled_run)
        } else {
            None
        };

        running_tasks.borrow_mut().insert(task.task_id.clone());

        let database = database.clone();
        let workers_controller = workers_controller.clone();
        let running_tasks = running_tasks.clone();

        task_futures.push(async move {
            let task_next_run = match task.schedule.after(&now).next() {
                Some(v) => v,
                None => {
                    // TODO: report this error to management thread or logs
                    error!("could not calculate next cron task run date (from now) according to schedule, skipping task from running");
                    return Utc::now() + Duration::from_secs(60); // retrying likely won't help, but let's schedule task one minute into future anyway
                },
            };

            let event_send_result = cron_events.send_async(CronTaskEvent::Start {
                name: task.name.clone(),
                function_id: task.function_id.clone(),
            }).await;
            if event_send_result.is_err() {
                // error here means that management thread was shut down. In that case it makes sense to stop executing cron tasks early
                // because cron thread will shut down too.
                info!("skipping running cron task because management thread shut down.");
                return task_next_run;
            }

            let iteration_delay = Instant::now() - iteration_start_time;

            let request_future = workers_controller.function_invoke(task.function_id.clone(), FetchRequestHeader::from_http_parts({
                let mut request = http::Request::new(());
                *request.method_mut() = http::Method::GET;
                *request.uri_mut() = match task.endpoint.as_deref().unwrap_or("/_fx/cron").parse() {
                    Ok(v) => v,
                    Err(err) => {
                        // TODO: report this error to management thread or logs
                        error!("cannot construct request to cron endpoint, invalid url: {err:?}");
                        return Utc::now() + Duration::from_secs(60); // retrying won't help, but let's schedule task one minute into future anyway
                    }
                };
                request
                    .into_parts()
                    .0
            }));
            let timeout_future = tokio::time::sleep(task.timeout.unwrap_or(Duration::from_secs(60)));

            let is_ok = tokio::select! {
                result = request_future => match result {
                    Ok(_) => true,
                    Err(err) => {
                        error!("failed to run function when executing cron task: {err:?}");
                        false
                    },
                },
                _ = timeout_future => {
                    error!("timeout while executing cron task: {:?}", task.task_id);
                    false
                },
            };

            let run_at = Utc::now();

            if is_ok && database.update_run_time(&task.task_id, run_at).is_err() {
                error!("failed to update run time in cron database for task. Task will run again.");
            }

            // error here means that management thread was shut down. In this future, we cannot control cron task (to shut it down in reaction
            // to management thread), so it makes sense to just ignore the error
            let _ = cron_events.send_async(CronTaskEvent::Run {
                name: task.name.clone(),
                function_id: task.function_id.clone(),
                run_at,
                delay,
                iteration_delay,
            }).await;

            running_tasks.borrow_mut().remove(&task.task_id);

            if is_ok {
                task_next_run
            } else {
                // if failed, try to re-run immediately
                Utc::now() + Duration::from_secs(1) // add some minimal delay to avoid running panicking function in a loop without stopping
            }
        }.boxed_local());
    }

    next_run
}
