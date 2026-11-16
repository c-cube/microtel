//! `/api` read endpoints and the embedded UI (SPA fallback included).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use include_dir::{Dir, include_dir};

use crate::query::{self, GroupBy};
use crate::store::Store;

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/ui/dist");

const MAX_RANGE_MS: u64 = 35 * 86_400_000;
const DEFAULT_RANGE_MS: u64 = 3_600_000;

pub fn router(store: Arc<Store>) -> Router {
    Router::new()
        .route("/api/spans/barchart", get(barchart))
        .route("/api/metrics/series", get(series))
        .route("/api/metrics/points", get(points))
        .route("/api/traces/{trace_id}", get(trace))
        .fallback(ui)
        .with_state(store)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn param<'a>(p: &'a HashMap<String, String>, key: &str) -> Option<&'a str> {
    p.get(key).map(String::as_str)
}

fn parse_range(p: &HashMap<String, String>) -> Result<(u64, u64), (StatusCode, &'static str)> {
    let to = match param(p, "to") {
        Some(s) => s
            .parse::<u64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid from/to"))?,
        None => now_ms(),
    };
    let from = match param(p, "from") {
        Some(s) => s
            .parse::<u64>()
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid from/to"))?,
        None => to.saturating_sub(DEFAULT_RANGE_MS),
    };
    if from == 0 || to <= from || to - from > MAX_RANGE_MS {
        return Err((StatusCode::BAD_REQUEST, "invalid from/to"));
    }
    Ok((from, to))
}

fn parse_services(p: &HashMap<String, String>) -> Vec<String> {
    param(p, "svcs")
        .unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

async fn barchart(
    State(store): State<Arc<Store>>,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    let (from, to) = match parse_range(&p) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let by = match GroupBy::parse(param(&p, "group_by").unwrap_or("service")) {
        Some(by) => by,
        None => return (StatusCode::BAD_REQUEST, "invalid group_by").into_response(),
    };
    let buckets = match bucket_param(&p) {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    Json(query::barchart(
        &store,
        from,
        to,
        by,
        &parse_services(&p),
        query::SCAN_CAP,
        buckets,
    ))
    .into_response()
}

async fn trace(
    State(store): State<Arc<Store>>,
    Path(trace_id): Path<String>,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    let at = match param(&p, "at") {
        Some(s) => match s.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid at").into_response(),
        },
        None => now_ms(),
    };
    match query::trace(&store, &trace_id, at) {
        Ok(spans) => Json(spans).into_response(),
        Err(query::TraceError::BadId) => {
            (StatusCode::BAD_REQUEST, "invalid trace id").into_response()
        }
        Err(query::TraceError::NotFound) => {
            (StatusCode::NOT_FOUND, "trace not found").into_response()
        }
    }
}

async fn series(State(store): State<Arc<Store>>) -> Response {
    Json(query::series(&store)).into_response()
}

async fn points(
    State(store): State<Arc<Store>>,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    let name = param(&p, "name").unwrap_or("");
    if name.is_empty() {
        return (StatusCode::BAD_REQUEST, "missing name").into_response();
    }
    let (from, to) = match parse_range(&p) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let buckets = match bucket_param(&p) {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    Json(query::points(
        &store,
        name,
        from,
        to,
        &parse_services(&p),
        query::SCAN_CAP,
        buckets,
    ))
    .into_response()
}

/// Optional `buckets` hint; the server clamps it to a sane range.
fn bucket_param(p: &HashMap<String, String>) -> Result<Option<usize>, (StatusCode, &'static str)> {
    match param(p, "buckets") {
        Some(s) => s
            .parse::<usize>()
            .map(Some)
            .map_err(|_| (StatusCode::BAD_REQUEST, "invalid buckets")),
        None => Ok(None),
    }
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

fn cache_control(path: &str) -> &'static str {
    if path.starts_with("assets/") {
        // vite content-hashes these filenames
        "public, max-age=31536000, immutable"
    } else if path.starts_with("vendor/") {
        "public, max-age=86400"
    } else {
        "no-cache"
    }
}

fn index_or_placeholder() -> Response {
    match DIST.get_file("index.html") {
        Some(file) => (
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            file.contents(),
        )
            .into_response(),
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            "ui not built: run `pnpm --dir ui build`",
        )
            .into_response(),
    }
}

async fn ui(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, "no such api route").into_response();
    }
    if let Some(file) = DIST.get_file(path) {
        return (
            [
                (header::CONTENT_TYPE, content_type(path)),
                (header::CACHE_CONTROL, cache_control(path)),
            ],
            file.contents(),
        )
            .into_response();
    }
    // Real assets 404; everything else is a client route.
    if path.contains('.') {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    index_or_placeholder()
}
