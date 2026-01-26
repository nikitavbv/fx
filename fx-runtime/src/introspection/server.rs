use {
    tracing::error,
    axum::{Router, routing::get, response::Response, Extension, http::StatusCode},
    leptos::prelude::*,
    crate::runtime::metrics::Metrics,
};

pub async fn run_introspection_server(runtime_metrics: Metrics) {
    let app = Router::new()
        .route("/", get(home))
        .route("/introspection", get(introspection))
        .route("/metrics", get(metrics))
        .layer(Extension(runtime_metrics));

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8081").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn home() -> Response {
    render_component(view! {
        <>
            <h2>"fx runtime"</h2>
            <br></br>
            <a href="/introspection">"/introspection"</a>" - realtime dashboard for troubleshooting and insights."<br></br>
            <a href="/metrics">"/metrics"</a>" - metrics exported in prometheus format."<br></br>
        </>
    })
}

async fn metrics(Extension(metrics): Extension<Metrics>) -> (StatusCode, String) {
    match metrics.encode() {
        Ok(v) => (StatusCode::OK, v),
        Err(err) => {
            error!("failed to encode metrics: {err:?}");
            (StatusCode::INTERNAL_SERVER_ERROR, "interal server error.\n".to_owned())
        }
    }
}

async fn introspection() -> Response {
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
                <span class="value">3</span>
              </div>
              <div class="summary-item">
                <span class="label">consumers:</span>
                <span class="value">2</span>
              </div>
              <div class="summary-item">
                <span class="label">pending futures:</span>
                <span class="value">847</span>
              </div>
            </div>

            <div class="alerts">
              <div class="alert alert-error">"consumer 'orders' stuck for 5m 23s"</div>
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
                  <tr>
                    <td>api</td>
                    <td><span class="status ok">ok</span></td>
                    <td class="number">184,729</td>
                    <td class="number">12</td>
                    <td class="number">2,847/min</td>
                    <td class="number">2.4ms</td>
                  </tr>
                  <tr>
                    <td>worker</td>
                    <td><span class="status ok">ok</span></td>
                    <td class="number">94,112</td>
                    <td class="number">0</td>
                    <td class="number">412/min</td>
                    <td class="number">45.2ms</td>
                  </tr>
                  <tr class="row-warn">
                    <td>notifier</td>
                    <td><span class="status degraded">degraded</span></td>
                    <td class="number">12,847</td>
                    <td class="number">847</td>
                    <td class="number">0/min</td>
                    <td class="number">124.8ms</td>
                  </tr>
                </tbody>
              </table>
            </section>

            <section>
              <h2>Consumers</h2>
              <table>
                <thead>
                  <tr>
                    <th>queue</th>
                    <th>function</th>
                    <th>handler</th>
                    <th>state</th>
                    <th>in state</th>
                    <th class="number">processed</th>
                    <th class="number">errors</th>
                  </tr>
                </thead>
                <tbody>
                  <tr>
                    <td>notifications</td>
                    <td>notifier</td>
                    <td>handle</td>
                    <td><span class="status waiting">waiting</span></td>
                    <td>4s</td>
                    <td class="number">94,112</td>
                    <td class="number">23</td>
                  </tr>
                  <tr class="row-error">
                    <td>orders</td>
                    <td>worker</td>
                    <td>process_order</td>
                    <td><span class="status stuck">stuck</span></td>
                    <td>5m 23s</td>
                    <td class="number">12,000</td>
                    <td class="number">0</td>
                  </tr>
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

fn render_component(component: impl IntoView + 'static) -> Response {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(component.to_html().into())
        .unwrap()
}
