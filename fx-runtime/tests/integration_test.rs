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
            BlobConfig,
            SqlBindingConfig,
            FunctionBindingConfig,
        },
        effects::logs::{EventFieldValue, LogEventType, BoxLogger},
    },
    crate::logger::TestLogger,
};

mod logger;

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
async fn http_body() {
    let client = init_fx_server().await;
    let response = client.post("/test/http/body").body("hello fx!".to_owned()).send().await.unwrap();
    assert!(response.status().is_success());
    assert_eq!("!xf olleh", response.text().await.unwrap());
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
    init_fx_server();

    let response = reqwest::get("http://localhost:8080/test/sql/migration-sql-error").await.unwrap();
    assert!(response.status().is_success());
    assert!(response.text().await.unwrap().contains("ok: migration sql error"));
}

// TODO: recover from panics?
#[tokio::test]
async fn function_panic() {
    let client = init_fx_server().await;
    let response = client.get("/test/panic").header("Host", "panics.fx.local").send().await.unwrap();
    assert_eq!(502, response.status().as_u16());
    assert_eq!("function panicked while handling request.\n", response.text().await.unwrap());
}

#[tokio::test]
async fn async_simple() {
    let client = init_fx_server().await;

    let started_at = Instant::now();
    let response = client.get("/test/sleep").send().await.unwrap();
    let total_time = (Instant::now() - started_at).as_secs();

    assert!(response.status().is_success());
    assert!(total_time >= 2);
    assert!(total_time <= 4);
}

#[tokio::test]
async fn async_conurrent() {
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
async fn kv_simple() {
    let client = init_fx_server().await;

    let result = client.get("/test/kv/simple").send().await.unwrap();
    assert!(result.status().is_success());
    assert_eq!("ok.", result.text().await.unwrap());
}

pub struct TestClient {
    client: reqwest::Client,
    base_url: String,
}

impl TestClient {
    fn new(base_url: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            base_url,
        }
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.get(&url)
    }

    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.post(&url)
    }

    pub fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.delete(&url)
    }

    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let url = format!("{}{}", self.base_url, path);
        self.client.request(method, &url)
    }
}

struct TestServer {
    #[allow(dead_code)]
    server: RunningFxServer,
    base_url: String,
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
                        functions_dir: "/tmp/fx/functions".to_owned(),
                        cron_data_path: None,
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
                            .with_code_inline(fs::read("../target/wasm32-unknown-unknown/release/fx_test_app.wasm").unwrap())
                            .with_binding_blob("test-blob".to_owned(), "test-blob-bucket".to_owned())
                            .with_binding_sql("app".to_owned(), ":memory:".to_owned())
                            .with_binding_sql("cron-test".to_owned(), ":memory:".to_owned())
                            .with_binding_sql_config(
                                SqlBindingConfig::new("contention-test".to_owned(), "/tmp/fx-test/contention.sqlite".to_owned())
                                    .with_busy_timeout_ms(10)
                            )
                            .with_binding_sql_config(
                                SqlBindingConfig::new("contention-busy".to_owned(), "/tmp/fx-test/contention-busy.sqlite".to_owned())
                                    .with_busy_timeout_ms(10)
                            )
                            .with_binding_sql_config(
                                SqlBindingConfig::new("nonexistent-db".to_owned(), "/tmp/fx-test/nonexistent/directory/test.sqlite".to_owned())
                            )
                            .with_binding_sql("migration-sql-error".to_owned(), ":memory:".to_owned())
                            .with_binding_function(FunctionBindingConfig::new(
                                "function-rpc".to_owned(),
                                "test-app-rpc".to_owned(),
                                Some("function-rpc.fx.local".to_owned())
                            ))
                            .with_binding_kv("test-namespace".to_owned(), "test-namespace".to_owned())
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
