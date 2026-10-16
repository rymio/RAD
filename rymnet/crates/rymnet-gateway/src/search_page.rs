use axum::{
    body::Body,
    http::{HeaderValue, StatusCode, header},
    response::Response,
};
use rymnet_search::{SearchResult, default_index_root, search};
use rymnet_storage::DataLayout;

/// A reserved, built-in domain: `search.rym` is not owned by anyone and is
/// never resolved through the normal claim+manifest+chunk pipeline. Every
/// gateway renders it identically, using whatever it has locally indexed
/// (its own published sites plus anything learned from peers via
/// `search.record.announce`/`rym search --network`).
pub const SEARCH_DOMAIN: &str = "search.rym";

pub fn render(layout: &DataLayout, query: Option<&str>) -> Response {
    let query = query.map(str::trim).filter(|q| !q.is_empty());
    let results = query
        .map(|q| {
            let index_root = default_index_root(layout.search_root());
            search(index_root, q, None, 20).unwrap_or_default()
        })
        .unwrap_or_default();

    let body = page_html(query.unwrap_or(""), &results);
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; script-src 'none'; object-src 'none'; frame-src 'none'; base-uri 'none';",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn page_html(query: &str, results: &[SearchResult]) -> String {
    let results_html = if query.is_empty() {
        String::new()
    } else if results.is_empty() {
        format!(
            r#"<p class="meta">No results for &quot;{}&quot;.</p>"#,
            escape(query)
        )
    } else {
        let mut html = format!(r#"<p class="meta">{} result(s)</p>"#, results.len());
        for result in results {
            let link = result_link(result);
            html.push_str(&format!(
                r#"<div class="result"><a href="{link}">{title}</a><div class="url">{url}</div><div class="desc">{desc}</div></div>"#,
                link = escape(&link),
                title = escape(&result.title),
                url = escape(&result.url),
                desc = escape(&result.description),
            ));
        }
        html
    };

    format!(
        r#"<!doctype html>
<html>
<head>
<meta charset="utf-8">
<title>search.rym</title>
<style>
body {{ font-family: -apple-system, sans-serif; max-width: 640px; margin: 3rem auto; color: #202124; padding: 0 1rem; }}
h1 {{ font-size: 2.4rem; text-align: center; font-weight: 400; }}
h1 .accent {{ color: #4285f4; }}
form {{ display: flex; justify-content: center; margin: 1.5rem 0 2.5rem; }}
input[type=text] {{ width: 60%; max-width: 420px; padding: 0.6rem 0.9rem; border: 1px solid #dfe1e5; border-radius: 24px 0 0 24px; font-size: 1rem; outline: none; }}
button {{ padding: 0.6rem 1.2rem; border: 1px solid #4285f4; background: #4285f4; color: #fff; border-radius: 0 24px 24px 0; font-size: 1rem; cursor: pointer; }}
.result {{ margin-bottom: 1.6rem; }}
.result a {{ font-size: 1.15rem; color: #1a0dab; text-decoration: none; }}
.result a:hover {{ text-decoration: underline; }}
.result .url {{ color: #006621; font-size: 0.82rem; margin: 0.1rem 0; }}
.result .desc {{ color: #4d5156; font-size: 0.92rem; }}
.meta {{ text-align: center; color: #70757a; font-size: 0.85rem; }}
footer {{ text-align: center; color: #999; font-size: 0.78rem; margin-top: 3rem; }}
</style>
</head>
<body>
<h1><span class="accent">search</span>.rym</h1>
<form method="get" action="/search.rym/">
<input type="text" name="q" value="{query_attr}" placeholder="Search rymnet..." autofocus>
<button type="submit">Search</button>
</form>
{results_html}
<footer>A node-based search engine built into every rymnet gateway. No JavaScript, no tracking, no central index.</footer>
</body>
</html>"#,
        query_attr = escape(query),
    )
}

fn result_link(result: &SearchResult) -> String {
    let suffix = if result.path == "/index.html" {
        "/".to_string()
    } else {
        result.path.clone()
    };
    format!("/{}{}", result.domain, suffix)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
