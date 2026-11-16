//! `/api` read endpoints and the embedded UI (SPA fallback included).

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use include_dir::{Dir, include_dir};

use crate::query::{self, GroupBy, GroupDimension};
use crate::stats::Stats;
use crate::store::Store;

static DIST: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/ui/dist");

const MAX_RANGE_MS: u64 = 35 * 86_400_000;
const DEFAULT_RANGE_MS: u64 = 3_600_000;

pub struct Api {
    pub store: Arc<Store>,
    pub stats: Arc<Stats>,
}

pub fn router(api: Arc<Api>) -> Router {
    Router::new()
        .route("/api/spans/barchart", get(barchart))
        .route("/api/spans/options", get(span_options))
        .route("/api/metrics/series", get(series))
        .route("/api/metrics/points", get(points))
        .route("/api/traces/{trace_id}", get(trace))
        .route("/health", get(health))
        .route("/stats", get(stats))
        .fallback(ui)
        .with_state(api)
}

async fn health() -> &'static str {
    "ok"
}

async fn stats(State(api): State<Arc<Api>>) -> String {
    api.stats.render(&api.store)
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
    parse_string_list(param(p, "svcs").unwrap_or(""))
}

fn parse_string_list(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_else(|_| {
        raw.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    })
}

fn parse_groups(p: &HashMap<String, String>) -> Result<Vec<GroupDimension>, ()> {
    let Some(raw) = param(p, "groups") else {
        return match GroupBy::parse(param(p, "group_by").unwrap_or("service")) {
            Some(GroupBy::None) => Ok(Vec::new()),
            Some(GroupBy::Service) => Ok(vec![GroupDimension::Service]),
            Some(GroupBy::Name) => Ok(vec![GroupDimension::Name]),
            None => Err(()),
        };
    };
    let keys: Vec<String> = serde_json::from_str(raw).map_err(|_| ())?;
    if keys.len() > 8 {
        return Err(());
    }
    let mut groups = Vec::with_capacity(keys.len());
    for key in keys {
        let group = GroupDimension::parse(&key).ok_or(())?;
        if groups.contains(&group) {
            continue;
        }
        groups.push(group);
    }
    Ok(groups)
}

fn parse_span_names(p: &HashMap<String, String>) -> Vec<String> {
    parse_string_list(param(p, "snames").unwrap_or(""))
}

async fn barchart(
    State(api): State<Arc<Api>>,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    let store = &api.store;
    let (from, to) = match parse_range(&p) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let groups = match parse_groups(&p) {
        Ok(groups) => groups,
        Err(()) => return (StatusCode::BAD_REQUEST, "invalid groups").into_response(),
    };
    let buckets = match bucket_param(&p) {
        Ok(b) => b,
        Err(e) => return e.into_response(),
    };
    Json(query::barchart_grouped(
        store,
        from,
        to,
        &groups,
        &parse_services(&p),
        &parse_span_names(&p),
        query::SCAN_CAP,
        buckets,
    ))
    .into_response()
}

async fn span_options(
    State(api): State<Arc<Api>>,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    let store = &api.store;
    let (from, to) = match parse_range(&p) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    Json(query::span_options(store, from, to, query::SCAN_CAP)).into_response()
}

async fn trace(
    State(api): State<Arc<Api>>,
    Path(trace_id): Path<String>,
    Query(p): Query<HashMap<String, String>>,
) -> Response {
    let store = &api.store;
    let at = match param(&p, "at") {
        Some(s) => match s.parse::<u64>() {
            Ok(v) => v,
            Err(_) => return (StatusCode::BAD_REQUEST, "invalid at").into_response(),
        },
        None => now_ms(),
    };
    match query::trace(store, &trace_id, at) {
        Ok(spans) => Json(spans).into_response(),
        Err(query::TraceError::BadId) => {
            (StatusCode::BAD_REQUEST, "invalid trace id").into_response()
        }
        Err(query::TraceError::NotFound) => {
            (StatusCode::NOT_FOUND, "trace not found").into_response()
        }
    }
}

async fn series(State(api): State<Arc<Api>>) -> Response {
    Json(query::series(&api.store)).into_response()
}

async fn points(State(api): State<Arc<Api>>, Query(p): Query<HashMap<String, String>>) -> Response {
    let store = &api.store;
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
        store,
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

async fn ui(method: Method, uri: Uri) -> Response {
    // the SPA fallback answers reads only: an OTLP POST aimed at the wrong
    // port must fail loudly instead of getting a 200 index.html
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
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
