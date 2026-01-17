use std::sync::{Arc, Mutex};

use axum::{Router, routing::get, response::Html, extract::State};
use rusqlite::Connection;
use yarte::Template;

struct Fortune {
    id: u64,
    message: String,
}

#[derive(Template)]
#[template(path = "fortunes.html.hbs")]
pub struct FortunesTemplate<'a> {
    pub fortunes: &'a Vec<Fortune>,
}

type Db = Arc<Mutex<Connection>>;

#[tokio::main]
async fn main() {
    let db = Arc::new(Mutex::new(Connection::open("./local/fortunes/fortunes.sqlite").unwrap()));

    let app = Router::new()
        .route("/fortunes", get(fortunes))
        .with_state(db);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await.unwrap();
    println!("listening on 8080");
    axum::serve(listener, app).await.unwrap();
}

async fn fortunes(State(db): State<Db>) -> Html<String> {
    let mut fortunes: Vec<Fortune> = {
        let conn = db.lock().unwrap();
        let mut stmt = conn.prepare("SELECT id, message FROM fortune").unwrap();
        stmt.query_map([], |row| {
            Ok(Fortune {
                id: row.get::<_, i64>(0)? as u64,
                message: row.get(1)?,
            })
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    };

    fortunes.push(Fortune {
        id: 0,
        message: "Additional fortune added at request time.".to_string(),
    });

    fortunes.sort_by(|a, b| a.message.cmp(&b.message));

    Html(FortunesTemplate { fortunes: &fortunes }.call().unwrap())
}
