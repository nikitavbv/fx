use {
    std::collections::HashMap,
    tracing::info,
    fx::{FxCtx, HttpRequest, HttpResponse, rpc},
    axum::{Router, routing::get, response::{Response, IntoResponse}, Extension},
    leptos::prelude::*,
    fx_utils::handle_http_axum_router,
    crate::{
        icons::{Settings, Code, Activity, Plus, Play, MoreHorizontal},
        components::{Button, ButtonVariant, Badge, BadgeVariant},
        cloud_api::FxCloudClient,
        database::Database,
    },
};

mod cloud_api;
mod components;
mod database;
mod events;
mod icons;

#[rpc]
pub async fn http(ctx: &FxCtx, req: HttpRequest) -> HttpResponse {
    ctx.init_logger();

    let database = Database::new(ctx.sql("dashboard")).await;
    database.run_migrations();

    let app = Router::new()
        .route("/", get(home))
        .route("/future", get(future))
        .layer(Extension(FxCloudClient::new()))
        .layer(Extension(database));

    handle_http_axum_router(app, req).await
}

#[derive(Clone)]
struct Function {
    id: String,
    total_invocations: u64,
}

async fn home(Extension(cloud): Extension<FxCloudClient>, Extension(database): Extension<Database>) -> impl IntoResponse {
    let functions = cloud.list_functions();
    let tracked_functions: HashMap<String, _> = database.list_functions().await.into_iter()
        .map(|v| (v.function_id.clone(), v))
        .collect();
    let functions = functions.into_iter()
        .map(|function| Function {
            total_invocations: tracked_functions.get(&function.id).map(|v| v.total_invocations).unwrap_or(0),
            id: function.id,
        })
        .collect();

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
                        <Button
                            variant=ButtonVariant::Ghost
                            class="justify-start bg-emerald-950 text-emerald-300">
                            <Code class="mr-2 h-4 w-4" />
                            Functions
                        </Button>
                        <Button
                            variant=ButtonVariant::Ghost
                            class="justify-start text-emerald-500 hover:bg-emerald-950 hover:text-emerald-300">
                            <Activity class="mr-2 h-4 w-4" />
                            Status
                        </Button>
                    </nav>
                </div>
                <div class="col-span-10 bg-black/95 p-6">
                    <div class="mb-6 flex items-center justify-between">
                        <h1 class="text-2xl font-bold tracking-light">Functions</h1>
                        <Button class="bg-emerald-700 text-black hover:bg-emerald-600">
                            <Plus class="mr-2 h-4 w-4" />
                            New Function
                        </Button>
                    </div>
                    <FunctionList functions />
                </div>
            </div>
        </div>
    })
}

async fn future() -> &'static str {
    info!("before sleep");
    fx::sleep().await;
    info!("after sleep");
    "ok.\n"
}

#[component]
fn function_list(functions: Vec<Function>) -> impl IntoView {
    view! {
        <div>
            <div class="border border-emerald-900/50 rounded-md overflow-hidden">
                <table class="w-full text-sm">
                    <thead>
                        <tr class="border-b border-emerald-900/50 bg-black/80">
                            <th class="px-4 py-3 text-left text-emerald-500">Name</th>
                            <th class="px-4 py-3 text-left text-emerald-500">Status</th>
                            <th class="px-4 py-3 text-left text-emerald-500">Invocations 24h</th>
                            <th class="px-4 py-3 text-left text-emerald-500">Errors 24h</th>
                            <th class="px-4 py-3 text-left text-emerald-500">CPU ops 24h</th>
                            <th class="px-4 py-3 text-left text-emerald-500">Memory</th>
                            <th class="px-4 py-3 text-left text-emerald-500">Execution duration p99 24h</th>
                            <th class="px-4 py-3 text-left text-emerald-500">Actions</th>
                        </tr>
                    </thead>
                    <tbody>
                        {
                            functions
                                .iter()
                                .enumerate()
                                .map(|(index, function)| view! {
                                    <FunctionListRow index={index as i64} function=function.clone() />
                                })
                                .collect::<Vec<_>>()
                        }
                    </tbody>
                </table>
            </div>
        </div>
    }
}

#[component]
fn function_list_row(index: i64, function: Function) -> impl IntoView {
    let tr_class = format!("border border-emerald-900/30 hover:bg-emerald-950/30 {}", if index % 2 == 0 { "bg-black/90" } else { "bg-black/70" });
    view! {
        <tr class=tr_class>
            <td class="px-4 py-3">{ function.id }</td>
            <td class="px-4 py-3"><Badge variant=BadgeVariant::Default class="bg-emerald-900/50 text-emerald-400 hover:bg-emerald-900/70">ok</Badge></td>
            <td class="px-4 py-3">{ function.total_invocations }</td>
            <td class="px-4 py-3">0</td>
            <td class="px-4 py-3">150M</td>
            <td class="px-4 py-3">2MB</td>
            <td class="px-4 py-3">210ms</td>
            <td class="px-4 py-3 text-right">
                <div class="flex items-center justify-end gap-2">
                    <Button
                        variant=ButtonVariant::Outline
                        class="h-7 w-7 p-0 border-emerald-900/50 bg-black text-emerald-400 hover:bg-emerald-950 hover:text-emerald-300">
                        <Play class="h-3 w-3" />
                        <span class="sr-only">Invoke</span>
                    </Button>
                    <Button
                        variant=ButtonVariant::Outline
                        class="h-7 w-7 p-0 border-emerald-900/50 bg-black text-emerald-400 hover:bg-emerald-950 hover:text-emerald-300">
                        <MoreHorizontal class="h-4 w-4" />
                        <span class="sr-only">More</span>
                    </Button>
                </div>
            </td>
        </tr>
    }
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
