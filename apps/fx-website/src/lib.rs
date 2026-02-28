use {
    fx_sdk::{HttpRequest, HttpResponse, handler, utils::axum::handle_request},
    axum::{
        Router,
        routing::get,
        response::{Html, IntoResponse},
        extract::Path,
        http::StatusCode,
    },
    include_dir::{include_dir, Dir},
};

static DOCS: Dir = include_dir!("$CARGO_MANIFEST_DIR/docs");

#[handler]
pub async fn http(req: HttpRequest) -> HttpResponse {
    handle_request(
        Router::new()
            .route("/", get(index))
            .route("/docs/runtime", get(docs_runtime_index))
            .route("/docs/runtime/{*path}", get(docs_runtime)),
        req,
    ).await
}

async fn index() -> Html<&'static str> {
    Html(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>fx runtime</title>
    <style>
        * {
            margin: 0;
            padding: 0;
            box-sizing: border-box;
        }
        body {
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Oxygen, Ubuntu, sans-serif;
            color: #e0e0e0;
            background: #0a0a0a;
            min-height: 100vh;
            display: flex;
            flex-direction: column;
        }
        header {
            padding: 1.5rem 2rem;
            border-bottom: 1px solid #1a1a1a;
        }
        header a {
            color: #e0e0e0;
            text-decoration: none;
            font-weight: 600;
            font-size: 1.1rem;
            font-family: 'SF Mono', 'Fira Code', 'Fira Mono', monospace;
        }
        main {
            flex: 1;
            display: flex;
            align-items: center;
            justify-content: center;
            padding: 2rem;
        }
        .hero {
            text-align: center;
            max-width: 640px;
        }
        .hero h1 {
            font-size: 3rem;
            font-weight: 700;
            margin-bottom: 1rem;
            font-family: 'SF Mono', 'Fira Code', 'Fira Mono', monospace;
        }
        .hero p {
            font-size: 1.2rem;
            color: #888;
            line-height: 1.6;
        }
    </style>
</head>
<body>
    <header>
        <a href="/">fx</a>
    </header>
    <main>
        <div class="hero">
            <h1>fx runtime</h1>
            <p>a minimal wasm application server</p>
        </div>
    </main>
</body>
</html>"#,
    )
}

async fn docs_runtime_index() -> impl IntoResponse {
    serve_doc_html("fx_runtime/index.html", "/docs/runtime/fx_runtime/")
}

async fn docs_runtime(Path(path): Path<String>) -> impl IntoResponse {
    let path = if path.ends_with('/') || path.is_empty() {
        format!("{}index.html", path)
    } else {
        path
    };

    match DOCS.get_file(&path) {
        Some(file) => (StatusCode::OK, file.contents().to_vec()).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

fn serve_doc_html(path: &str, base: &str) -> impl IntoResponse {
    match DOCS.get_file(path) {
        Some(file) => {
            let html = String::from_utf8_lossy(file.contents());
            let base_tag = format!(r#"<base href="{base}">"#);
            (StatusCode::OK, Html(html.replacen("<head>", &format!("<head>{base_tag}"), 1))).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}
