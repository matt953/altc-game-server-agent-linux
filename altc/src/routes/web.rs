use axum::{
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
};
use rust_embed::Embed;

/// The built UI, compiled into the binary rather than read from disk.
///
/// It has to render at first boot with no network and nothing to fetch — it is
/// how the server gets claimed, so anything it depends on being downloaded is
/// a way for setup to fail before it starts.
#[derive(Embed)]
#[folder = "web-dist"]
struct Assets;

pub async fn serve(uri: Uri) -> Response {
    // An unmatched API path must answer as an API. Falling through to the SPA
    // hands a JSON client an HTML page, and the resulting parse error shows up
    // a long way from the typo that caused it.
    if uri.path().starts_with("/api/") {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "application/json")],
            r#"{"error":"no such endpoint"}"#,
        )
            .into_response();
    }

    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response()
        }
        // Anything unrecognised falls back to the app, so client-side routes
        // survive a refresh. A missing API route is handled before this.
        None => match Assets::get("index.html") {
            Some(index) => ([(header::CONTENT_TYPE, "text/html")], index.data).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                "the web UI was not built into this binary",
            )
                .into_response(),
        },
    }
}
