//! Встроенный веб-интерфейс (web/dist). В релизной сборке файлы внутри бинарника,
//! в отладочной читаются с диска. Неизвестные пути отдают index.html (SPA).

use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../../web/dist"]
#[allow_missing = true]
struct Assets;

const NOT_BUILT: &str = r#"<!doctype html><meta charset="utf-8"><title>Сканер карт</title>
<body style="font:16px system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem">
<h1>Интерфейс не собран</h1>
<p>API работает на <code>/api</code>. Соберите интерфейс и перезапустите сервер:</p>
<pre>cd web &amp;&amp; npm install &amp;&amp; npm run build</pre>
<p>Для разработки: <code>npm run dev</code> в папке <code>web</code> и откройте адрес Vite.</p></body>"#;

fn file(path: &str, f: rust_embed::EmbeddedFile) -> Response {
    let mime = mime_guess::from_path(path).first_or_octet_stream();
    let cache = if path.starts_with("assets/") { "public, max-age=31536000, immutable" } else { "no-cache" };
    ([(header::CONTENT_TYPE, mime.as_ref().to_string()), (header::CACHE_CONTROL, cache.to_string())], f.data.into_owned()).into_response()
}

pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "нет такого метода API").into_response();
    }
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(f) = Assets::get(path) {
        return file(path, f);
    }
    match Assets::get("index.html") {
        Some(f) => file("index.html", f),
        None => Html(NOT_BUILT).into_response(),
    }
}
