use fx::{handler, HttpRequest, SqlQuery, HttpResponse, HeaderValue};

struct Fortune {
    id: u64,
    message: String,
}

#[handler]
pub async fn fortunes(req: HttpRequest) -> fx::Result<HttpResponse> {
    let db = fx::sql("fortunes");
    let mut fortunes = db.exec(SqlQuery::new("select id, message from fortune"))
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

    Ok(
        HttpResponse::new()
            .with_header("Content-Type", HeaderValue::from_static("text/html; charset=utf-8"))
            .with_body(render_html(&fortunes))
    )
}

fn render_html(fortunes: &Vec<Fortune>) -> String {
    "".to_owned()
}
