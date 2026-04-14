use {
    std::{fs, sync::{Arc, OnceLock}, time::Instant, env::current_dir},
    once_cell::sync::Lazy,
    tokio::{join, sync::{OnceCell, Mutex}, time::{sleep, Duration}},
    futures::{StreamExt, stream::FuturesUnordered, FutureExt},
    fx_runtime::{
        FxServer,
        server::RunningFxServer,
        function::FunctionId,
        config::{
            ServerConfig,
            HttpServerConfig,
            ServerPort,
            FunctionConfig,
            LoggerConfig,
            SqlConfig,
            BlobConfig,
            SqlBindingConfig,
            FunctionBindingConfig,
            LimitsConfig,
            IntrospectionConfig,
            FunctionCronTriggerConfig,
            EnvVariableConfig,
        },
        effects::logs::{EventFieldValue, LogEventType, BoxLogger},
    },
    crate::common::{
        logger::TestLogger,
        utils::{TestClient, TestServer},
    },
};

mod common;

static LOGGER: Lazy<Arc<TestLogger>> = Lazy::new(|| Arc::new(TestLogger::new()));
static LOGGER_CUSTOM_FUNCTION: Lazy<Arc<TestLogger>> = Lazy::new(|| Arc::new(TestLogger::new()));

#[tokio::test]
async fn simple() {
    let client = init_fx_server().await;
    let response = client.get("/").send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("hello fx!", response.text().await.unwrap());
}

#[tokio::test]
async fn test_http_header() {
    let client = init_fx_server().await;
    let response = client
        .get("/test/http/header-get-simple")
        .header("x-test-header", "some header value")
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!("ok: some header value\n", response.text().await.unwrap());
}

#[tokio::test]
async fn test_http_response_header() {
    let client = init_fx_server().await;
    let response = client.get("/test/http/response-header").send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("test-value", response.headers().get("x-custom-header").unwrap().to_str().unwrap());
}

#[tokio::test]
async fn uri_overwrite() {
    let client = init_fx_server().await;
    let response = client.get("/test/http/uri-overwrite").send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("hello from overwritten uri", response.text().await.unwrap());
}

#[tokio::test]
async fn status_code() {
    let client = init_fx_server().await;
    let response = client.get("/test/status-code").send().await.unwrap();
    assert_eq!(418, response.status().as_u16());
}

#[tokio::test]
async fn http_head() {
    let client = init_fx_server().await;
    let response = client.request(reqwest::Method::HEAD, "/").send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("", response.text().await.unwrap());
}

#[tokio::test]
async fn http_body() {
    let client = init_fx_server().await;
    let response = client.post("/test/http/body").body("hello fx!".to_owned()).send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("!xf olleh", response.text().await.unwrap());
}

#[tokio::test]
async fn http_reserved_namespace() {
    let client = init_fx_server().await;
    let response = client.get("/_fx/cron").send().await.unwrap();
    assert_eq!(404, response.status().as_u16());
    assert_eq!("not found.\n", response.text().await.unwrap());
}

#[tokio::test]
async fn sql_simple() {
    let client = init_fx_server().await;
    let response = client.get("/test/sql/simple").send().await.unwrap();
    assert_eq!("52", response.text().await.unwrap());
}

#[tokio::test]
async fn sql_migrate() {
    let client = init_fx_server().await;
    let response = client.get("/test/sql/migrate").send().await.unwrap();
    assert_eq!("67", response.text().await.unwrap());
}

#[tokio::test]
async fn sql_batch() {
    let client = init_fx_server().await;
    let response = client.get("/test/sql/batch").send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("sum=600", response.text().await.unwrap());
}

#[tokio::test]
async fn sql_batch_rollback() {
    let client = init_fx_server().await;
    let response = client.get("/test/sql/batch-rollback").send().await.unwrap();
    let status = response.status();
    let text = response.text().await.unwrap();
    assert!(status.is_success(), "Expected success but got: {}", text);
    assert_eq!("rollback verified", text);
}

/// This test verifies that database behaves correctly in a simple contention scenario:
/// - we have one expensive write query
/// - we have a lot of inexpensive reads
/// write query should not block reads.
/// In practice this means that this test can only pass if sqlite has WAL enabled.
#[tokio::test]
async fn sql_simple_contention() {
    let client = init_fx_server().await;

    let result = client.get("/test/sql/contention/setup").send().await.unwrap();
    assert!(result.status().is_success());

    let client_ref = &client;
    let futures: FuturesUnordered<_> = std::iter::once(client_ref.get("/test/sql/contention/expensive-write").send().boxed())
        .chain((0..20).map(|_| async move {
            sleep(Duration::from_millis(5)).await;
            client_ref.get("/test/sql/contention/read").send().await
        }.boxed()))
        .collect();
    let results: Vec<_> = futures.collect().await;

    for response in results {
        let response = response.unwrap();
        assert!(response.status().is_success());
        assert!(response.text().await.unwrap().starts_with("ok.\n"));
    }
}

/// It is still possible to lock database, because of slow disk for example.
/// In that case, DatabaseBusy error should propagate to application and be handled correctly.
#[tokio::test]
async fn sql_contention_busy() {
    let client = init_fx_server().await;

    let client_ref = &client;
    let futures: FuturesUnordered<_> = (0..20).map(|n| async move {
        sleep(Duration::from_millis(n)).await;
        let result = client_ref.get("/test/sql/contention-busy").send().await.unwrap();
        assert!(result.status().is_success());
        result.text().await.unwrap()
    }).collect();
    let results: Vec<_> = futures.collect().await;

    assert!(results.contains(&"ok.\n".to_owned()));
    assert!(results.contains(&"busy.\n".to_owned()));
}

#[tokio::test]
async fn sql_wrong_binding_name() {
    let client = init_fx_server().await;

    let response = client.get("/test/sql/wrong-binding-name").send().await.unwrap();
    assert!(response.status().is_success());
    assert!(response.text().await.unwrap().contains("ok: binding not found"));
}

#[tokio::test]
async fn sql_wrong_binding_name_migrations() {
    let client = init_fx_server().await;

    let response = client.get("/test/sql/wrong-binding-name/migrations").send().await.unwrap();
    assert!(response.status().is_success());
    assert!(response.text().await.unwrap().contains("ok: binding not found"));
}

#[tokio::test]
async fn sql_migration_sql_error() {
    let client = init_fx_server().await;

    let response = client.get("/test/sql/migration-sql-error").send().await.unwrap();
    assert!(response.status().is_success());
    assert!(response.text().await.unwrap().contains("ok: migration sql error"));
}

#[tokio::test]
async fn function_panic() {
    let client = init_fx_server().await;
    let response = client.get("/test/panic").header("Host", "panics.fx.local").send().await.unwrap();
    assert_eq!(502, response.status().as_u16());
    assert_eq!("function panicked while handling request.\n", response.text().await.unwrap());
}

#[tokio::test]
async fn function_panic_loop() {
    let client = init_fx_server().await;

    // check that everything will work correctly even if a lot of panics happen in the loop
    for _ in 0..50 {
        // check that function is actually functional between panics
        let response = client.get("/").header("Host", "panics.fx.local").send().await.unwrap();
        assert!(response.status().is_success());

        // trigger function panic
        let response = client.get("/test/panic").header("Host", "panics.fx.local").send().await.unwrap();
        assert_eq!(502, response.status().as_u16());
        assert_eq!("function panicked while handling request.\n", response.text().await.unwrap());
    }
}

#[tokio::test]
async fn panic_restore() {
    let client = init_fx_server().await;

    // check that everything will work correctly even if a lot of panics happen in the loop
    for _ in 0..50 {
        // check that function is actually functional between panics and it has "fresh state" with panic counter = 0
        let response = client.get("/test/panic-restore/test").header("Host", "panics.fx.local").send().await.unwrap();
        assert!(response.status().is_success());
        assert_eq!("panic counter: 0", response.text().await.unwrap());

        // trigger function panic
        let response = client.get("/test/panic-restore").header("Host", "panics.fx.local").send().await.unwrap();
        assert_eq!(502, response.status().as_u16());
        assert_eq!("function panicked while handling request.\n", response.text().await.unwrap());
    }
}

#[tokio::test]
async fn async_simple() {
    let client = init_fx_server().await;

    let started_at = Instant::now();
    let response = client.get("/test/sleep").send().await.unwrap();
    let total_time = (Instant::now() - started_at).as_secs();

    let status = response.status();
    assert!(status.is_success(), "received response: {status:?}. Response body: {:?}", response.text().await.unwrap());
    assert!(total_time >= 2);
    assert!(total_time <= 4);
}

#[tokio::test]
async fn async_concurrent() {
    let client = init_fx_server().await;

    let started_at = Instant::now();
    let client_ref = &client;
    let _ = join!(
        async move {
            let response = client_ref.get("/test/sleep").send().await.unwrap();
            assert!(response.status().is_success());
        },
        async move {
            let response = client_ref.get("/test/sleep").send().await.unwrap();
            assert!(response.status().is_success());
        },
    );
    let total_time = (Instant::now() - started_at).as_secs();
    assert!(total_time >= 2);
    assert!(total_time <= 4);
}

#[tokio::test]
async fn test_random() {
    let client = init_fx_server().await;

    let response1 = client.get("/test/random").send().await.unwrap().text().await.unwrap();
    let response2 = client.get("/test/random").send().await.unwrap().text().await.unwrap();
    assert!(response1 != response2);
    assert!((30..50).contains(&response1.len()));
    assert!((30..50).contains(&response2.len()));
}

#[tokio::test]
async fn test_time() {
    let client = init_fx_server().await;

    let millis: u64 = client.get("/test/time").send().await.unwrap()
        .text().await.unwrap()
        .parse().unwrap();
    assert!((950..=1050).contains(&millis));
}

#[tokio::test]
async fn blob_simple() {
    let client = init_fx_server().await;

    let result = client.delete("/test/blob").send().await.unwrap();
    assert!(result.status().is_success());

    let result = client.get("/test/blob").send().await.unwrap();
    assert_eq!(404, result.status().as_u16());

    let result = client.post("/test/blob").send().await.unwrap();
    assert_eq!(200, result.status().as_u16());

    let result = client.get("/test/blob").send().await.unwrap();
    assert!(result.status().is_success());
    assert_eq!("test-value", result.text().await.unwrap());

    let result = client.delete("/test/blob").send().await.unwrap();
    assert!(result.status().is_success());
}

#[tokio::test]
async fn blob_wrong_binding_name() {
    let client = init_fx_server().await;

    let result = client.get("/test/blob/wrong-binding-name").send().await.unwrap();
    assert!(result.status().is_success());
}

#[tokio::test]
async fn fetch() {
    let client = init_fx_server().await;

    let result = client.get("/test/fetch").send().await.unwrap();
    assert!(result.status().is_success());
    assert!(result.text().await.unwrap().contains("httpbin.org/get"));
}

#[tokio::test]
async fn fetch_post() {
    let client = init_fx_server().await;

    let result = client.get("/test/fetch/post").send().await.unwrap();
    assert!(result.status().is_success());
    let result = result.text().await.unwrap();
    assert!(result.contains("httpbin.org/post"));
    assert!(result.contains("test fx request body"));
}

#[tokio::test]
async fn fetch_body_passthrough() {
    let client = init_fx_server().await;

    let result = client.post("/test/fetch/body-passthrough").body("fx test: body passthrough").send().await.unwrap();
    assert!(result.status().is_success());
    let result = result.text().await.unwrap();
    assert!(result.contains("httpbin.org/post"));
    assert!(result.contains("fx test: body passthrough"));
}

#[tokio::test]
async fn fetch_query() {
    let client = init_fx_server().await;

    let result = client.get("/test/fetch/query").send().await.unwrap();
    assert!(result.status().is_success());
    let result = result.text().await.unwrap();
    assert!(result.contains("https://httpbin.org/get?param1=value1&param2=value2"));
}

#[tokio::test]
async fn fetch_with_header() {
    let client = init_fx_server().await;

    let result = client.get("/test/fetch/with-header").send().await.unwrap();
    assert!(result.status().is_success());
    let result = result.text().await.unwrap();
    assert!(result.contains("X-Custom-Header"));
    assert!(result.contains("custom-value"));
}

#[tokio::test]
async fn log() {
    let client = init_fx_server().await;

    let result = client.get("/test/log").header("Host", "custom-logger.fx.local").send().await.unwrap();
    assert!(result.status().is_success());

    let mut events = Vec::new();
    for _ in 0..10 {
        events = LOGGER.events();
        let found_expected_event = events.iter()
            .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("this is a test log".to_owned())).unwrap_or(false))
            .is_some();
        if found_expected_event {
            return;
        } else {
            // logs are processed asynchronously, so there can be a delay
            sleep(Duration::from_secs(1)).await;
            continue;
        }
    }

    panic!("didn't find expected event. All events: {events:?}");
}

#[tokio::test]
async fn log_span() {
    let client = init_fx_server().await;

    let result = client.get("/test/log/span").header("Host", "custom-logger.fx.local").send().await.unwrap();
    assert!(result.status().is_success());

    // both events include fields inherited from span
    let first_message = LOGGER.events()
        .into_iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("first message".to_owned())).unwrap_or(false))
        .expect("expected first message to be present");
    let second_message = LOGGER.events()
        .into_iter()
        .find(|v| v.fields.get("message").map(|v| v == &EventFieldValue::Text("second message".to_owned())).unwrap_or(false))
        .expect("expected second message to be present");
    assert!(first_message.fields.get("request_id").unwrap() == &EventFieldValue::Text("some-request-id".to_owned()));
    assert!(second_message.fields.get("request_id").unwrap() == &EventFieldValue::Text("some-request-id".to_owned()));

    // span begin and end are logged
    assert!(
        LOGGER.events()
            .into_iter()
            .find(|v| v.event_type == LogEventType::Begin && v.fields.get("name").map(|v| v == &EventFieldValue::Text("test_log_span".to_owned())).unwrap_or(false))
            .is_some()
    );
    assert!(
        LOGGER.events()
            .into_iter()
            .find(|v| v.event_type == LogEventType::End && v.fields.get("name").map(|v| v == &EventFieldValue::Text("test_log_span".to_owned())).unwrap_or(false))
            .is_some()
    );
}

#[tokio::test]
async fn metrics_counter_increment() {
    let client = init_fx_server().await;

    let result = client.get("/test/metrics/counter-increment").send().await.unwrap();
    assert!(result.status().is_success());

    for _ in 0..10 {
        let result = match reqwest::get("http://localhost:9000/metrics").await {
            Ok(v) => v,
            Err(err) => {
                if err.is_connect() {
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
                panic!("unexpected error when querying /metrics: {err:?}");
            }
        };

        assert!(result.status().is_success());

        let result = result.text().await.unwrap();
        if !result.contains("function_test_app_test_counter") {
            sleep(Duration::from_secs(1)).await;
            continue;
        }

        assert!(result.contains("# TYPE function_test_app_test_counter counter\n"));
        assert!(result.contains("function_test_app_test_counter 1\n"));
        return;
    }

    panic!("failed to check if counter value is present in /metrics");
}

#[tokio::test]
async fn metrics_counter_with_labels_increment() {
    let client = init_fx_server().await;

    let result = client.get("/test/metrics/counter-with-labels-increment").send().await.unwrap();
    assert!(result.status().is_success());

    for _ in 0..10 {
        let result = match reqwest::get("http://localhost:9000/metrics").await {
            Ok(v) => v,
            Err(err) => {
                if err.is_connect() {
                    sleep(Duration::from_secs(1)).await;
                    continue;
                }
                panic!("unexpected error when querying /metrics: {err:?}");
            }
        };

        assert!(result.status().is_success());

        let result = result.text().await.unwrap();
        if !result.contains("function_test_app_test_counter_with_label") {
            sleep(Duration::from_secs(1)).await;
            continue;
        }

        assert!(result.contains("function_test_app_test_counter_with_label{label_name=\"value1\"} 1\n"));
        assert!(result.contains("function_test_app_test_counter_with_label{label_name=\"value2\"} 2\n"));
    }
}

#[tokio::test]
async fn invalid_host_header() {
    let client = init_fx_server().await;

    // Send a request with invalid UTF-8 bytes in the Host header (0x80 is not valid UTF-8)
    let invalid_host = vec![104, 111, 115, 116, 46, 0x80, 46, 99, 111, 109]; // "host.<invalid>.com"
    let response = client.get("/")
        .header(reqwest::header::HOST, reqwest::header::HeaderValue::from_bytes(&invalid_host).unwrap())
        .send()
        .await
        .unwrap();

    assert_eq!(400, response.status().as_u16());
    assert_eq!("invalid Host header\n", response.text().await.unwrap());
}

#[tokio::test]
async fn function_remove() {
    let client = init_fx_server().await;

    // intially, function should be live
    let result = client.get("/")
        .header("Host", "for-remove.fx.local")
        .send()
        .await
        .unwrap();

    assert!(result.status().is_success());
    let result = result.text().await.unwrap();
    assert_eq!("hello from function to remove!", result);

    // remove function (uses management API on port 9000)
    let result = reqwest::Client::new().delete("http://localhost:9000/api/functions/test-app-for-remove")
        .send()
        .await
        .unwrap();
    assert!(result.status().is_success());

    // check that function is not available anymore
    let result = client.get("/")
        .header("Host", "for-remove.fx.local")
        .send()
        .await
        .unwrap();

    // response comes from default function now
    assert!(result.status().is_success());
    assert_eq!("hello fx!", result.text().await.unwrap());
}

#[tokio::test]
async fn cron_simple() {
    let client = init_fx_server().await;

    let result1 = client.get("/test/cron").send().await
        .unwrap()
        .text()
        .await
        .unwrap()
        .parse::<u64>()
        .unwrap();

    sleep(Duration::from_secs(4)).await;

    let result2 = client.get("/test/cron").send().await
        .unwrap()
        .text()
        .await
        .unwrap()
        .parse::<u64>()
        .unwrap();

    assert!(result2 >= result1 + 3); // in 4 seconds we expect cron job to run three times
}

#[tokio::test]
async fn cron_custom_endpoint() {
    let client = init_fx_server().await;

    let result1 = client.get("/test/cron/custom-endpoint").send().await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(result1.contains("cron with custom endpoint:"));
    let result1 = result1
        .replace("cron with custom endpoint: ", "")
        .parse::<u64>()
        .unwrap();

    sleep(Duration::from_secs(4)).await;

    let result2 = client.get("/test/cron/custom-endpoint").send().await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(result2.contains("cron with custom endpoint:"));
    let result2 = result2
        .replace("cron with custom endpoint: ", "")
        .parse::<u64>()
        .unwrap();

    assert!(result2 >= result1 + 3); // in 4 seconds we expect cron job to run three times
}

#[tokio::test]
async fn rpc_simple() {
    let client = init_fx_server().await;

    let result = client.get("/test/rpc/simple").send().await.unwrap();

    assert!(result.status().is_success());
    assert_eq!("rpc test: hello from other function!", result.text().await.unwrap());
}

#[tokio::test]
async fn sql_binding_nonexistent_directory() {
    let client = init_fx_server().await;
    let response = client.get("/test/sql/nonexistent-db").send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("ok: 1", response.text().await.unwrap());
}

#[tokio::test]
async fn response_stream_simple() {
    let client = init_fx_server().await;

    let started_at = Instant::now();
    let response = client.get("/test/stream/sse").send().await.unwrap();
    assert!(response.status().is_success());

    let mut response = response.bytes_stream();
    let mut buffer = String::new();
    let mut events = Vec::new();

    while let Some(chunk) = response.next().await {
        buffer.push_str(&String::from_utf8(chunk.unwrap().into()).unwrap());

        while let Some(pos) = buffer.find("\n\n") {
            let event_str = buffer[..pos].to_string();
            buffer = buffer[pos + 2..].to_string();

            if event_str.starts_with("data: ") {
                let data = event_str[6..].to_string();
                events.push((Instant::now(), data));
            }
        }
    }

    assert_eq!(events.len(), 5, "Expected 5 events, got {}", events.len());
    for (i, (_, event)) in events.iter().enumerate() {
        assert_eq!(event, &format!("Message {}", i + 1));
    }

    let first_event_time = events[0].0.duration_since(started_at).as_millis();
    assert!(
        first_event_time >= 900 && first_event_time <= 1500,
        "first event should arrive after ~1 second, but took {}ms",
        first_event_time
    );

    for i in 1..events.len() {
        let interval = events[i].0.duration_since(events[i - 1].0).as_millis();
        assert!(
            interval >= 900 && interval <= 1500,
            "event {} should arrive ~1 second after event {}, but interval was {}ms",
            i + 1,
            i,
            interval
        );
    }

    let total_time = Instant::now().duration_since(started_at).as_secs();
    assert!(
        total_time >= 4 && total_time <= 7,
        "total streaming time should be around 5 seconds, but was {} seconds",
        total_time
    );
}

#[tokio::test]
async fn env_simple() {
    let client = init_fx_server().await;

    let result = client.get("/test/env/simple").send().await.unwrap();
    assert!(result.status().is_success());
    assert_eq!("ok.", result.text().await.unwrap());
}

#[tokio::test]
async fn env_missing_file() {
    assert!(init_fx_server().await.get("/test/env/missing-file").send().await.unwrap().status().is_success());
}

#[tokio::test]
async fn kv_simple() {
    let client = init_fx_server().await;

    let result = client.get("/test/kv/simple").send().await.unwrap();
    assert!(result.status().is_success());
    assert_eq!("ok.", result.text().await.unwrap());
}

#[tokio::test]
async fn kv_distributed_lock() {
    let client = init_fx_server().await;

    let (result1, result2) = join!(
        client.get("/test/kv/distributed-lock").send(),
        client.get("/test/kv/distributed-lock").send(),
    );
    let result1 = result1.unwrap();
    let result2 = result2.unwrap();

    assert!(result1.status().is_success());
    assert!(result2.status().is_success());

    let result1 = result1.text().await.unwrap();
    let result2 = result2.text().await.unwrap();

    assert!((result1 == "ok.\n" && result2 == "already locked.\n") || (result2 == "ok.\n" && result1 == "already locked.\n"));
}

#[tokio::test]
async fn kv_pubsub() {
    let client = init_fx_server().await;

    let (result, _, _, _) = join!(
        client.get("/test/kv/pubsub/subscribe").send(),
        async {
            sleep(Duration::from_millis(100)).await;
            client.post("/test/kv/pubsub/publish").json(&serde_json::json!({"value": 42})).send().await.unwrap();
        },
        async {
            sleep(Duration::from_millis(100)).await;
            client.post("/test/kv/pubsub/publish").json(&serde_json::json!({"value": 43})).send().await.unwrap();
        },
        async {
            sleep(Duration::from_millis(100)).await;
            client.post("/test/kv/pubsub/publish").json(&serde_json::json!({"value": 44})).send().await.unwrap();
        },
    );
    let result = result.unwrap();

    assert!(result.status().is_success(), "status = {}", result.status().as_u16());
    assert_eq!("result: 129", result.text().await.unwrap());
}

#[tokio::test]
async fn task_background() {
    let client = init_fx_server().await;

    // before task is started, kv key does not exist
    let status = client.get("/test/tasks/background/status").send().await.unwrap();
    assert!(status.status().as_u16() == 404);

    // start task
    let task = client.post("/test/tasks/background/start").send().await.unwrap();
    assert!(task.status().is_success());

    // after task is started, kv key does not exist immediately
    let status = client.get("/test/tasks/background/status").send().await.unwrap();
    assert!(status.status().as_u16() == 404);

    // but after some time, it should be present
    sleep(Duration::from_secs(2)).await;
    let status = client.get("/test/tasks/background/status").send().await.unwrap();
    assert!(status.status().is_success());
}

#[tokio::test]
async fn test_limits_memory() {
    let client = init_fx_server().await;

    let result = client.get("/test/limits/memory").header("Host", "limit-memory.fx.local").send().await.unwrap();
    assert!(result.status().is_server_error());
    assert_eq!("function panicked while handling request.\n", result.text().await.unwrap());
}

#[tokio::test]
async fn preemption() {
    let test_port = 8081;

    // using separate server instance to avoid affecting other tests
    let server = FxServer::new(ServerConfig {
        config_path: Some("/tmp/fx".into()),
        server: HttpServerConfig {
            port: ServerPort { value: test_port },
        },
        workers: Some(1),
        functions_dir: "/tmp/fx/functions".to_owned(),
        cron_data_path: None,
        sql: Some(SqlConfig {
            path: "/tmp/fx/sql".parse().unwrap(),
        }),
        blob: Some(BlobConfig {
            path: "/tmp/fx/blob".parse().unwrap(),
        }),
        logger: None,
        introspection: Some(IntrospectionConfig {
            enabled: false,
        }),
    }).start();

    server.deploy_function(
        FunctionId::new("test-app"),
        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
            .with_trigger_http(None)
    ).await;

    server.deploy_function(
        FunctionId::new("other-app"),
        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
            .with_trigger_http(Some("other-app.fx.local".to_owned()))
    ).await;
    server.deploy_function(
        FunctionId::new("other-app2"),
        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
            .with_trigger_http(Some("other-app2.fx.local".to_owned()))
    ).await;

    let client = TestClient::new(format!("http://localhost:{}", test_port));

    async fn make_slow_request(client: &TestClient) -> Duration {
        let started_at = Instant::now();
        let result = client.get("/test/limits/cpu-preemption").send().await.unwrap();
        assert!(result.status().is_success());
        assert!((Instant::now() - started_at).as_millis() > 1000, "slow request should be slow enough to test cpu preemption");
        Instant::now() - started_at
    }

    async fn make_fast_request(client: &TestClient) -> Duration {
        sleep(Duration::from_millis(100)).await;
        let started_at = Instant::now();
        let result = client.get("/").header("Host", "other-app.fx.local").send().await.unwrap();
        assert!(result.status().is_success());
        Instant::now() - started_at
    }

    let normal_fast = make_fast_request(&client).await;

    let (slow1, slow2, fast1, fast2, fast3) = join!(
        make_slow_request(&client),
        make_slow_request(&client),
        make_fast_request(&client),
        make_fast_request(&client),
        make_fast_request(&client),
    );

    // verify that slow requests are in fact slow
    assert!(slow1.as_millis() >= 1000);
    assert!(slow2.as_millis() >= 1000);

    // verify that fast requests are consistently fast even though runtime is busy with slow requests
    assert!(fast1.as_millis() <= 25, "fast request1 took {} ms, while normal is {}ms", fast1.as_millis(), normal_fast.as_millis());
    assert!(fast2.as_millis() <= 25, "fast request2 took {} ms, while normal is {}ms", fast2.as_millis(), normal_fast.as_millis());
    assert!(fast3.as_millis() <= 25, "fast request3 took {} ms, while normal is {}ms", fast3.as_millis(), normal_fast.as_millis());
}

async fn init_fx_server() -> TestClient {
    static TEST_SERVER: OnceLock<TestServer> = OnceLock::new();

    let test_server = TEST_SERVER.get_or_init(|| {
        let test_port: u16 = 8080;

        std::thread::spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(async {
                    let server = FxServer::new(ServerConfig {
                        config_path: Some("/tmp/fx".into()),
                        server: HttpServerConfig {
                            port: ServerPort { value: test_port },
                        },
                        workers: None,
                        functions_dir: "/tmp/fx/functions".to_owned(),
                        cron_data_path: None,
                        sql: Some(SqlConfig {
                            path: "/tmp/fx/sql".parse().unwrap(),
                        }),
                        blob: Some(BlobConfig {
                            path: "/tmp/fx/blob".parse().unwrap(),
                        }),
                        logger: Some(LoggerConfig::Custom(Arc::new(BoxLogger::new(LOGGER.clone())))),
                        introspection: None,
                    }).start();

                    server.deploy_function(
                        FunctionId::new("test-app"),
                        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                            .with_env("test-env-var".to_owned(), "test value".to_owned())
                            .with_trigger_http(None)
                            .with_trigger_cron("test-cron-job".to_owned(), "* * * * * *".to_owned())
                            .with_trigger_cron_config(FunctionCronTriggerConfig::new("test-cron-job-custom-endpoint".to_owned(),  "* * * * * *".to_owned()).with_endpoint("/_fx/cron/custom-endpoint-for-task"))
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                            .with_binding_blob("test-blob".to_owned(), "test-blob-bucket".to_owned())
                            .with_binding_sql_config(SqlBindingConfig::new("app".to_owned(), "app".to_owned(), true))
                            .with_binding_sql_config(SqlBindingConfig::new("cron-test".to_owned(), "cron-test".to_owned(), true))
                            .with_binding_sql_config(
                                SqlBindingConfig::new("contention-test".to_owned(), "contention".to_owned(), false)
                                    .with_busy_timeout_ms(10)
                            )
                            .with_binding_sql_config(
                                SqlBindingConfig::new("contention-busy".to_owned(), "contention-busy".to_owned(), false)
                                    .with_busy_timeout_ms(10)
                            )
                            .with_binding_sql_config(
                                SqlBindingConfig::new("nonexistent-db".to_owned(), "test".to_owned(), false)
                            )
                            .with_binding_sql_config(SqlBindingConfig::new("migration-sql-error".to_owned(), "migration-sql-error".to_owned(), true))
                            .with_binding_function(FunctionBindingConfig::new(
                                "function-rpc".to_owned(),
                                "test-app-rpc".to_owned(),
                                Some("function-rpc.fx.local".to_owned())
                            ))
                            .with_binding_kv("test-namespace".to_owned(), "test-namespace".to_owned())
                            .with_env_config(EnvVariableConfig::file("TEST_FILE_ENV_VAR", "/tmp/fx/test-file-that-does-not-exist"))
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-app-custom-logger"),
                        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                            .with_trigger_http(Some("custom-logger.fx.local".to_owned()))
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                            .with_logger(LoggerConfig::Custom(Arc::new(BoxLogger::new(LOGGER.clone()))))
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-app-for-panics"),
                        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                            .with_trigger_http(Some("panics.fx.local".to_owned()))
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-wrong-import"),
                        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_wrong_import.wasm").unwrap())
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-missing-export"),
                        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_missing_export.wasm").unwrap())
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-app-for-remove"),
                        FunctionConfig::new("/tmp/fx/functions/test-app-for-remove.fx.yaml".into())
                            .with_trigger_http(Some("for-remove.fx.local".to_owned()))
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_remove.wasm").unwrap())
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-app-rpc"),
                        FunctionConfig::new("/tmp/fx/functions/test-app-rpc.fx.yaml".into())
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app_rpc.wasm").unwrap())
                    ).await;

                    server.deploy_function(
                        FunctionId::new("test-app-limit-memory"),
                        FunctionConfig::new("/tmp/fx/functions/test-app.fx.yaml".into())
                            .with_trigger_http(Some("limit-memory.fx.local".to_owned()))
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                            .with_limits(LimitsConfig::new().with_memory_bytes(128 * 1024 * 1024))
                    ).await;

                    TestServer {
                        server,
                        base_url: format!("http://localhost:{}", test_port),
                    }
                })
        }).join().unwrap()
    });

    TestClient::new(test_server.base_url.clone())
}

// TODO: add test that verifies that counter metrics with labels are recorded correctly
// TODO: sql transactions
// TODO: test that database can only be accessed by correct binding name
// TODO: test sql with all types
// TODO: test sql with sqlx
// TODO: test sql with error
// TODO: test a lot of async calls in a loop with random response times to verify that multiple concurrent requests are handled correctly
// TODO: test what happens if function responds with incorrect type
// TODO: test compiler error
// TODO: test request bodies
