use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, get, post};
use prost::Message;
use tokio::sync::mpsc;

use crate::keys;
use crate::pipeline::Pipeline;
use crate::proto::{
    AttrSet, Data, ExportMetricsServiceRequest, ExportTraceServiceRequest, NumberDataPoint,
    SpanEnvelope, hash16,
};
use crate::stats::{Endpoint, Stats};
use crate::store::{MetricBatch, PointRow, SeriesRow, SpanBatch, SpanRow, Store};

pub struct App {
    pub traces: Pipeline<SpanBatch>,
    pub metrics: Pipeline<MetricBatch>,
    pub stats: Arc<Stats>,
    pub store: Arc<Store>,
}

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/v1/traces", post(traces))
        .route("/v1/metrics", post(metrics))
        .route("/v1/logs", any(logs))
        .route("/health", get(health))
        .route("/stats", get(stats))
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .with_state(app)
}

async fn traces(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    let resp = traces_inner(&app, &headers, body).await;
    app.stats.request(Endpoint::Traces, resp.status().as_u16());
    resp
}

async fn traces_inner(app: &App, headers: &HeaderMap, body: Bytes) -> Response {
    if let Some(resp) = check_content_type(headers) {
        return resp;
    }
    let raw = match decompress(headers, &body) {
        Ok(raw) => raw,
        Err(_) => return bad_request("invalid gzip body"),
    };
    let req = match ExportTraceServiceRequest::decode(raw.as_slice()) {
        Ok(req) => req,
        Err(_) => return bad_request("unparseable protobuf"),
    };
    let mut batch = SpanBatch {
        resources: Vec::new(),
        scopes: Vec::new(),
        spans: Vec::new(),
    };
    let mut res_rows: HashMap<[u8; keys::HASH_LEN], Vec<u8>> = HashMap::new();
    let mut scope_rows: HashMap<[u8; keys::HASH_LEN], Vec<u8>> = HashMap::new();
    let mut seen = 0u64;
    let mut invalid = 0u64;
    for rs in req.resource_spans {
        let resource = rs.resource.unwrap_or_default();
        let rh = hash16(&resource);
        res_rows
            .entry(rh)
            .or_insert_with(|| resource.encode_to_vec());
        for ss in rs.scope_spans {
            let scope = ss.scope.unwrap_or_default();
            let sh = hash16(&scope);
            scope_rows
                .entry(sh)
                .or_insert_with(|| scope.encode_to_vec());
            for span in ss.spans {
                seen += 1;
                // OTLP: ids of any other length or all zeroes are invalid
                let zero = |b: &Vec<u8>| b.iter().all(|&x| x == 0);
                if span.trace_id.len() != keys::TRACE_ID_LEN
                    || span.span_id.len() != keys::SPAN_ID_LEN
                    || zero(&span.trace_id)
                    || zero(&span.span_id)
                {
                    tracing::warn!("dropping span with invalid ids");
                    invalid += 1;
                    continue;
                }
                let trace_id = span.trace_id.clone();
                let span_id = span.span_id.clone();
                let start_ns = span.start_time_unix_nano;
                let ymd = keys::ymd(start_ns);
                let envelope = SpanEnvelope {
                    resource_hash: rh.to_vec(),
                    scope_hash: sh.to_vec(),
                    span: Some(span),
                }
                .encode_to_vec();
                batch.spans.push(SpanRow {
                    ymd,
                    trace_id,
                    span_id,
                    start_ns,
                    envelope,
                });
            }
        }
    }
    batch.resources = res_rows.into_iter().collect();
    batch.scopes = scope_rows.into_iter().collect();
    match app.traces.send(batch) {
        Ok(()) => {
            app.stats.spans_received(seen);
            app.stats.spans_dropped(invalid);
            ok_protobuf()
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            (StatusCode::TOO_MANY_REQUESTS, "queue full").into_response()
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            (StatusCode::SERVICE_UNAVAILABLE, "shutting down").into_response()
        }
    }
}

async fn metrics(State(app): State<Arc<App>>, headers: HeaderMap, body: Bytes) -> Response {
    let resp = metrics_inner(&app, &headers, body).await;
    app.stats.request(Endpoint::Metrics, resp.status().as_u16());
    resp
}

async fn metrics_inner(app: &App, headers: &HeaderMap, body: Bytes) -> Response {
    if let Some(resp) = check_content_type(headers) {
        return resp;
    }
    let raw = match decompress(headers, &body) {
        Ok(raw) => raw,
        Err(_) => return bad_request("invalid gzip body"),
    };
    let req = match ExportMetricsServiceRequest::decode(raw.as_slice()) {
        Ok(req) => req,
        Err(_) => return bad_request("unparseable protobuf"),
    };
    let mut batch = MetricBatch {
        series: Vec::new(),
        points: Vec::new(),
    };
    let mut series_rows: HashMap<(String, [u8; keys::HASH_LEN]), SeriesRow> = HashMap::new();
    let mut seen = 0u64;
    for rm in req.resource_metrics {
        for sm in rm.scope_metrics {
            for m in sm.metrics {
                let name = m.name.clone();
                let dps: Vec<NumberDataPoint> = match m.data {
                    Some(Data::Gauge(g)) => g.data_points,
                    Some(Data::Sum(s)) => s.data_points,
                    _ => {
                        tracing::debug!(name = %name, "metric type not stored");
                        continue;
                    }
                };
                seen += dps.len() as u64;
                for dp in dps {
                    let value = dp.encode_to_vec();
                    let attr_set = AttrSet {
                        attributes: dp.attributes,
                    };
                    let attr_hash = hash16(&attr_set);
                    let ymd = keys::ymd(dp.time_unix_nano);
                    series_rows
                        .entry((name.clone(), attr_hash))
                        .or_insert_with(|| SeriesRow {
                            name: name.clone(),
                            attr_hash,
                            value: attr_set.encode_to_vec(),
                        });
                    batch.points.push(PointRow {
                        name: name.clone(),
                        attr_hash,
                        ymd,
                        time_ns: dp.time_unix_nano,
                        value,
                    });
                }
            }
        }
    }
    batch.series = series_rows.into_values().collect();
    match app.metrics.send(batch) {
        Ok(()) => {
            app.stats.points_received(seen);
            ok_protobuf()
        }
        Err(mpsc::error::TrySendError::Full(_)) => {
            (StatusCode::TOO_MANY_REQUESTS, "queue full").into_response()
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            (StatusCode::SERVICE_UNAVAILABLE, "shutting down").into_response()
        }
    }
}

async fn logs(State(app): State<Arc<App>>) -> Response {
    let resp = (StatusCode::NOT_IMPLEMENTED, "logs signal not supported").into_response();
    app.stats.request(Endpoint::Logs, resp.status().as_u16());
    resp
}

async fn health() -> &'static str {
    "ok"
}

async fn stats(State(app): State<Arc<App>>) -> String {
    app.stats.render(&app.store)
}

/// 415 on an explicitly wrong content type; absent header defers to body parsing.
fn check_content_type(headers: &HeaderMap) -> Option<Response> {
    let ct = headers.get(header::CONTENT_TYPE)?.to_str().ok()?;
    let base = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if base == "application/x-protobuf" || base == "application/protobuf" {
        None
    } else {
        Some((StatusCode::UNSUPPORTED_MEDIA_TYPE, "want protobuf").into_response())
    }
}

/// `Content-Encoding: gzip` is authoritative; without the header, a gzip body is
/// still decompressed (with a warning) because clients disagree about advertising it.
fn decompress(headers: &HeaderMap, body: &Bytes) -> std::io::Result<Vec<u8>> {
    let declared = headers
        .get(header::CONTENT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|e| e.trim().eq_ignore_ascii_case("gzip")));
    let magic = body.starts_with(&[0x1f, 0x8b]);
    if declared {
        gunzip(body)
    } else if magic {
        tracing::warn!("gzip body without content-encoding: gzip");
        gunzip(body)
    } else {
        Ok(body.to_vec())
    }
}

fn gunzip(body: &Bytes) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(body.len() * 4);
    flate2::read::MultiGzDecoder::new(&body[..]).read_to_end(&mut out)?;
    Ok(out)
}

fn ok_protobuf() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/x-protobuf")],
        Vec::<u8>::new(),
    )
        .into_response()
}

fn bad_request(msg: &'static str) -> Response {
    (StatusCode::BAD_REQUEST, msg).into_response()
}
