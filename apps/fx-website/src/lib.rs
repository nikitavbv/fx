use {
    fx_sdk::{HttpRequest, HttpResponse, handler, utils::axum::handle_request},
    axum::{Router, routing::get, response::Html},
};

#[handler::fetch]
pub async fn http(req: HttpRequest) -> HttpResponse {
    handle_request(
        Router::new()
            .route("/", get(index)),
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
        </div>
    </main>
</body>
</html>"#,
    )
}
