//! `rad://localhost/v1/documents/{id}/(content|thumb)` — streams vault files from the server to
//! previews (PDF, images, audio, video) without exposing credentials to the webview.

use tauri::http::{header, Request, Response, StatusCode};
use tauri::{Manager, UriSchemeContext, UriSchemeResponder};

use crate::App;

fn allowed(path: &str) -> bool {
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    matches!(parts.as_slice(), ["v1", "documents", id, "content" | "thumb"] if id.parse::<i64>().is_ok())
}

pub fn handle<R: tauri::Runtime>(ctx: UriSchemeContext<'_, R>, req: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    tauri::async_runtime::spawn(async move {
        let path = req.uri().path().to_string();
        let fail = |status: StatusCode| Response::builder().status(status).body(Vec::new()).unwrap();
        if !allowed(&path) {
            responder.respond(fail(StatusCode::FORBIDDEN));
            return;
        }
        let st = app.state::<App>();
        let Ok(mut rb) = st.client.request(&st.client.stream_http, reqwest::Method::GET, &path) else {
            responder.respond(fail(StatusCode::SERVICE_UNAVAILABLE));
            return;
        };
        if let Some(r) = req.headers().get(header::RANGE) {
            rb = rb.header(header::RANGE.as_str(), r.as_bytes());
        }
        let resp = match rb.send().await {
            Ok(r) => r,
            Err(_) => {
                responder.respond(fail(StatusCode::BAD_GATEWAY));
                return;
            }
        };
        let status = resp.status().as_u16();
        let mut out = Response::builder().status(status).header("Access-Control-Allow-Origin", "*");
        for h in ["content-type", "content-range", "accept-ranges", "content-disposition", "cache-control"] {
            if let Some(v) = resp.headers().get(h) {
                out = out.header(h, v.as_bytes());
            }
        }
        let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
        responder.respond(out.body(body).unwrap_or_else(|_| fail(StatusCode::INTERNAL_SERVER_ERROR)));
    });
}
