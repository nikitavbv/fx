use {
    std::sync::Arc,
    serde::Deserialize,
    send_wrapper::SendWrapper,
    axum::{Router, routing::{get, delete}, response::Response, Extension, extract},
    leptos::prelude::*,
    crate::{
        tasks::{
            management::runtime_state::RuntimeState,
            worker::WorkersController,
        },
        effects::metrics::MetricsRegistry,
        function::FunctionId,
    },
};

pub(crate) async fn run_introspection_server(metrics: Arc<MetricsRegistry>, workers_controller: WorkersController, runtime_state: SendWrapper<RuntimeState>) {
    let app = Router::new()
        .route("/", get(introspection_home))
        .route("/metrics", get(introspection_metrics))
        .route("/api/functions/{function_id}", delete(management_api_function_remove))
        .route("/introspection", get(introspection))
        .layer(Extension(metrics))
        .layer(Extension(Arc::new(workers_controller)))
        .layer(Extension(runtime_state));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:9000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn introspection_home() -> Response {
    render_component(view! {
        <>
            <h2>"fx runtime"</h2>
            <br></br>
            <a href="/introspection">"/introspection"</a>" - realtime dashboard for troubleshooting and insights."<br></br>
            <a href="/metrics">"/metrics"</a>" - metrics exported in prometheus format."<br></br>
        </>
    })
}

async fn introspection_metrics(Extension(metrics): Extension<Arc<MetricsRegistry>>) -> String {
    metrics.encode()
}

fn render_component(component: impl IntoView + 'static) -> Response {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(component.to_html().into())
        .unwrap()
}

#[derive(Deserialize)]
struct FunctionIdPathArgument {
    function_id: String,
}

async fn management_api_function_remove(
    Extension(workers_controller): Extension<Arc<WorkersController>>,
    extract::Path(function_id): extract::Path<FunctionIdPathArgument>
) -> &'static str {
    workers_controller.function_remove(&FunctionId::new(&function_id.function_id)).await;
    "ok.\n"
}

async fn introspection(Extension(runtime_state): Extension<SendWrapper<RuntimeState>>) -> Response {
    let functions = runtime_state.functions();
    let function_count = functions.len();

    let function_rows: Vec<leptos::prelude::AnyView> = if functions.is_empty() {
        vec![view! {
            <tr>
                <td colspan="6" class="empty-state">"no functions deployed"</td>
            </tr>
        }.into_any()]
    } else {
        functions.iter().map(|f| view! {
            <tr>
                <td>{f.as_str()}</td>
                <td><span class="status ok">"ok"</span></td>
                <td class="number">"-"</td>
                <td class="number">"-"</td>
                <td class="number">"-"</td>
                <td class="number">"-"</td>
            </tr>
        }.into_any()).collect()
    };

    render_component(view! {
        <!DOCTYPE html>
        <html>
        <head>
          <title>fx introspection</title>
          <meta charset="utf-8"></meta>
          <script src="https://unpkg.com/htmx.org@2.0.4"></script>
          <style>{ include_str!("./style.css") }</style>
          <script>{ include_str!("./script.js") }</script>
        </head>
        <body hx-select="#content" hx-target="#content" hx-swap="outerHTML">
          <div id="content">
            <header>
              <h1>fx introspection</h1>
              <div class="controls">
                polling every 2s
              </div>
            </header>

            <div class="summary">
              <div class="summary-item">
                <span class="label">uptime:</span>
                <span class="value">3d 4h 12m</span>
              </div>
              <div class="summary-item">
                <span class="label">functions:</span>
                <span class="value">{function_count}</span>
              </div>
              <div class="summary-item">
                <span class="label">pending futures:</span>
                <span class="value">847</span>
              </div>
            </div>

            <div class="alerts">
              <div class="alert alert-warn">"function 'notifier' error rate: 6.5%"</div>
            </div>

            <section>
              <h2>Functions</h2>
              <table>
                <thead>
                  <tr>
                    <th>name</th>
                    <th>state</th>
                    <th class="number">invocations</th>
                    <th class="number">errors</th>
                    <th class="number">last 1min</th>
                    <th class="number">avg latency</th>
                  </tr>
                </thead>
                <tbody>
                  {function_rows}
                </tbody>
              </table>
            </section>

            <section>
              <h2>HTTP</h2>
              <table>
                <thead>
                  <tr>
                    <th>port</th>
                    <th>function</th>
                    <th>state</th>
                    <th class="number">active connections</th>
                    <th class="number">total requests</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>8080</td>
                    <td>api</td>
                    <td><span class="status listening">listening</span></td>
                    <td class="number">24</td>
                    <td class="number">184,729</td>
                  </tr>
                </tbody>
              </table>
            </section>

            <section>
              <h2>Cron</h2>
              <table>
                <thead>
                  <tr>
                    <th>task</th>
                    <th>function</th>
                    <th>schedule</th>
                    <th>last run</th>
                    <th>next run</th>
                    <th>state</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>cleanup</td>
                    <td>worker</td>
                    <td>0 * * * *</td>
                    <td>12m ago</td>
                    <td>in 48m</td>
                    <td><span class="status idle">idle</span></td>
                  </tr>
                  <tr>
                    <td>daily-report</td>
                    <td>notifier</td>
                    <td>0 9 * * *</td>
                    <td>6h ago</td>
                    <td>in 18h</td>
                    <td><span class="status idle">idle</span></td>
                  </tr>
                </tbody>
              </table>
            </section>

            <footer>
              fx runtime
            </footer>
          </div>
        </body>
        </html>
    })
}
