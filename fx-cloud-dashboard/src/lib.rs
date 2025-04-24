use {
    fx::{FxCtx, HttpRequest, HttpResponse, rpc},
    axum::{Router, routing::get, response::{Response, IntoResponse}},
    leptos::prelude::*,
    fx_utils::handle_http_axum_router,
    crate::{icons::{Settings, Code}, components::{Button, ButtonVariant}},
};

mod components;
mod icons;

#[rpc]
pub fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    let app = Router::new()
        .route("/", get(home))
        .route("/something", get(something));

    handle_http_axum_router(app, req)
}

async fn home() -> impl IntoResponse {
    render_page(view! {
        <div class="flex min-h-screen flex-col bg-black text-emerald-400">
            <header class="border-b border-emerald-900/50 bg-black/90 px-6 py-3">
                <div class="flex items-center justify-between">
                    <div class="flex items-center gap-2 text-lg font-bold tracking-wider">fx</div>
                    <div class="flex items-center gap-4">
                        <div class="flex items-center gap-2">
                            <div class="h-2 w-2 rounded-full bg-emerald-500 animate-pulse" />
                            <span class="text-xs">OK</span>
                        </div>
                        <Button
                            variant=ButtonVariant::Outline
                            class="border-emerald-700 bg-black text-emerald-400 hover:bg-emerald-950 hover:text-emerald-300">
                            <Settings class="mr-2 h-4 w-4" />Settings
                        </Button>
                    </div>
                </div>
            </header>
            <div class="grid flex-1 grid-cols-12 gap-0">
                <div class="col-span-2 border-r border-emerald-900/50 bg-black/90 p-4">
                    <nav class="flex flex-col gap-2">
                        { /* for selected menu option, use bg-emerald-950 text-emerald-300 */ }
                        <Button
                            variant=ButtonVariant::Ghost
                            class="justify-start text-emerald-500 hover:bg-emerald-950 hover:text-emerald-300">
                            <Code class="mr-2 h-4 w-4" />
                            Functions
                        </Button>
                    </nav>
                </div>
            </div>
        </div>
    })
}

async fn something() -> impl IntoResponse {
    "something"
}

fn render_page(page_component: impl IntoView + 'static) -> Response {
    render_component(view! {
        <!doctype html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <title>fx</title>
                <link rel="preconnect" href="https://fonts.googleapis.com" />
                <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
                <link href="https://fonts.googleapis.com/css2?&family=Space+Mono:ital,wght@0,400;0,700;1,400;1,700&display=swap" rel="stylesheet" />
                <link rel="stylesheet" href="/app.css" />
                <script src="https://unpkg.com/htmx.org@2.0.4"></script>
                <script src="https://cdn.tailwindcss.com"></script>
            </head>
            <body class="font-[family-name:Space_Mono]">{ page_component }</body>
        </html>
    })
}

fn render_component(component: impl IntoView + 'static) -> Response {
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(component.to_html().into())
        .unwrap()
}
