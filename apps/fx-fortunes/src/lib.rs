use {
    fx_sdk::{self as fx, handler, HttpRequest, SqlQuery, HttpResponse, io::http::HeaderValue},
    yarte::Template,
};

struct Fortune {
    id: u64,
    message: String,
}

#[derive(Template)]
#[template(path = "fortunes.html.hbs")]
pub struct FortunesTemplate<'a> {
    pub fortunes: &'a Vec<Fortune>,
}

#[fx::handler::fetch]
pub async fn http(_req: HttpRequest) -> handler::FunctionResponse {
    let db = fx::sql("fortunes");
    let mut fortunes = db.exec(SqlQuery::new("select id, message from fortune"))
        .await
        .unwrap()
        .into_rows()
        .into_iter()
        .map(|v| Fortune {
            id: (&v.columns[0]).try_into().unwrap(),
            message: (&v.columns[1]).try_into().unwrap(),
        })
        .collect::<Vec<_>>();
    fortunes.push(Fortune { id: 0, message: "Additional fortune added at request time.".to_owned() });
    fortunes.sort_by(|a, b| a.message.cmp(&b.message));

    /*Ok(
        HttpResponse::new()
            .with_header("Content-Type", HeaderValue::from_static("text/html; charset=utf-8"))
            .with_body(render_html(&fortunes))
    );*/

    unimplemented!()
}

fn render_html(fortunes: &Vec<Fortune>) -> String {
    FortunesTemplate { fortunes }.call().unwrap()
}
