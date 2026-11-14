mod common;

use std::path::PathBuf;
use std::time::Duration;

use prost::Message;
use rocksdb::IteratorMode;

const A_TRACES: usize = 6;
const B_TRACES: usize = 4;
// node spans: root+children = 6*(4+1) + 4*(3+1) = 46; rust requests add 2 valid spans
const EXPECTED_SPANS: usize = 48;
const EXPECTED_RESOURCES: usize = 3;
const EXPECTED_SERIES_MIN: usize = 5;
const EXPECTED_POINTS_MIN: usize = 7;

#[tokio::test]
async fn ingest_roundtrip() {
    // sandbox may have a read-only /tmp; keep tempdirs inside the repo
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("microtel-ingest-")
        .tempdir_in(&repo_tmp)
        .unwrap();
    let data_dir = dir.path().join("data");
    let (mut child, addr) = common::spawn_server(&data_dir, None);
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();
    let pb = "application/x-protobuf";

    let t0 = common::now_ns();

    // valid request: 1 stored span + 1 dropped (zero trace_id)
    let dropped_span = opentelemetry_proto::tonic::trace::v1::Span {
        name: "dropped".into(),
        start_time_unix_nano: t0,
        end_time_unix_nano: t0 + 1_000_000,
        ..Default::default()
    };
    let req = common::trace_request(vec![
        common::span([1u8; 16], [1u8; 8], "root-ok", t0),
        dropped_span,
    ]);
    let resp = client
        .post(format!("{base}/v1/traces"))
        .header(reqwest::header::CONTENT_TYPE, pb)
        .body(req.encode_to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.headers()["content-type"], pb);
    let body = resp.bytes().await.unwrap();
    assert!(body.is_empty(), "success response must be an empty proto");

    // gzip'd trace request: 1 more span
    let req = common::trace_request(vec![common::span([2u8; 16], [1u8; 8], "root-gzip", t0)]);
    let resp = client
        .post(format!("{base}/v1/traces"))
        .header(reqwest::header::CONTENT_TYPE, pb)
        .body(common::gzip(&req.encode_to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // gzip'd metrics request: 2 series, 2 points
    let req = common::metrics_request(vec![
        common::gauge_metric("rust_gauge", vec![common::kv("env", "test")], t0),
        common::sum_metric("rust_sum", t0),
    ]);
    let resp = client
        .post(format!("{base}/v1/metrics"))
        .header(reqwest::header::CONTENT_TYPE, pb)
        .body(common::gzip(&req.encode_to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // rejection paths
    let resp = client
        .post(format!("{base}/v1/logs"))
        .body("[]")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 501);
    let resp = client
        .post(format!("{base}/v1/traces"))
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 415);
    let resp = client
        .post(format!("{base}/v1/traces"))
        .header(reqwest::header::CONTENT_TYPE, pb)
        .body(b"definitely-not-proto".to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    common::run_emitter(&addr, A_TRACES, B_TRACES);

    // let the writer drain its last interval, then shut down gracefully
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let status = common::terminate(&mut child);
    assert!(
        status.success(),
        "graceful shutdown exited nonzero: {status}"
    );

    let db = common::open_read_only(&data_dir);

    let trace_rows = common::cf_count(&db, "spans_by_trace");
    let time_rows = common::cf_count(&db, "spans_by_time");
    assert_eq!(trace_rows, EXPECTED_SPANS, "spans_by_trace rows");
    // rows collapse when spans share (start_ns, trace_id); total entries must not
    assert!(time_rows <= EXPECTED_SPANS, "spans_by_time rows");
    assert_eq!(common::cf_count(&db, "resources"), EXPECTED_RESOURCES);
    assert!(
        common::cf_count(&db, "series") >= EXPECTED_SERIES_MIN,
        "series rows"
    );
    assert!(
        common::cf_count(&db, "points") >= EXPECTED_POINTS_MIN,
        "points rows"
    );

    // envelopes decode and reference real resource rows
    let res_cf = db.cf_handle("resources").unwrap();
    let env_cf = db.cf_handle("spans_by_trace").unwrap();
    let mut resource_hashes: std::collections::HashSet<Vec<u8>> = Default::default();
    for kv in db.iterator_cf(res_cf, IteratorMode::Start).flatten() {
        resource_hashes.insert(kv.0.to_vec());
        opentelemetry_proto::tonic::resource::v1::Resource::decode(&kv.1[..])
            .expect("resource proto decodes");
    }
    let mut envelope_checks = 0;
    for kv in db.iterator_cf(env_cf, IteratorMode::Start).flatten() {
        let envelope = common::SpanEnvelope::decode(&kv.1[..]).expect("envelope decodes");
        let span = envelope.span.as_ref().expect("envelope carries a span");
        assert_eq!(&kv.0[4..20], &span.trace_id[..], "key embeds trace_id");
        assert_eq!(&kv.0[20..28], &span.span_id[..], "key embeds span_id");
        assert!(
            resource_hashes.contains(&envelope.resource_hash),
            "envelope resource hash resolves"
        );
        envelope_checks += 1;
    }
    assert_eq!(envelope_checks, EXPECTED_SPANS);

    // time index keys are strictly ascending by start_ns
    let time_cf = db.cf_handle("spans_by_time").unwrap();
    let mut time_entries = 0usize;
    let mut last_ns = 0u64;
    for kv in db.iterator_cf(time_cf, IteratorMode::Start).flatten() {
        let ns = u64::from_be_bytes(kv.0[4..12].try_into().unwrap());
        assert!(ns >= last_ns, "time index not sorted: {ns} < {last_ns}");
        assert_eq!(kv.0.len(), 4 + 8 + 16, "time key layout");
        let count = (kv.1[0] & 0x7f) as usize; // varint count, small
        assert_eq!(kv.1.len(), 1 + count * 8, "time value layout");
        time_entries += count;
        last_ns = ns;
    }
    assert_eq!(time_entries, EXPECTED_SPANS, "time index span entries");

    // series keys parse as len-prefixed names + 16B attr hash
    let series_cf = db.cf_handle("series").unwrap();
    let mut names = std::collections::HashSet::new();
    for kv in db.iterator_cf(series_cf, IteratorMode::Start).flatten() {
        let (len, n) = read_varint(&kv.0);
        let name = std::str::from_utf8(&kv.0[n..n + len]).expect("metric name is utf8");
        names.insert(name.to_string());
        assert_eq!(kv.0.len(), n + len + 16, "series key layout");
    }
    for expected in [
        "rust_gauge",
        "rust_sum",
        "requests_total",
        "queue_depth",
        "errors_total",
    ] {
        assert!(names.contains(expected), "series missing {expected}");
    }
}

/// minimal varint reader mirroring src/keys.rs
fn read_varint(buf: &[u8]) -> (usize, usize) {
    let mut value = 0usize;
    let mut shift = 0;
    for (i, &b) in buf.iter().enumerate() {
        value |= ((b & 0x7f) as usize) << shift;
        if b & 0x80 == 0 {
            return (value, i + 1);
        }
        shift += 7;
    }
    unreachable!("varint too long")
}

const PB: &str = "application/x-protobuf";

async fn post(
    client: &reqwest::Client,
    base: &str,
    path: &str,
    ct: Option<&str>,
    body: Vec<u8>,
) -> reqwest::Response {
    let mut req = client.post(format!("{base}{path}"));
    if let Some(ct) = ct {
        req = req.header(reqwest::header::CONTENT_TYPE, ct);
    }
    req.body(body).send().await.unwrap()
}

#[tokio::test]
async fn protocol_edges() {
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("microtel-proto-")
        .tempdir_in(&repo_tmp)
        .unwrap();
    let data_dir = dir.path().join("data");
    let (mut child, addr) = common::spawn_server(&data_dir, None);
    let base = format!("http://{addr}");
    // no keep-alive reuse: the oversized-body request below gets its connection
    // aborted, and a pooled dead connection would break the next request
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(0)
        .build()
        .unwrap();
    let t0 = common::now_ns();

    // content-type variants are all accepted
    let req = common::trace_request(vec![common::span([11u8; 16], [1u8; 8], "charset", t0)]);
    let resp = post(
        &client,
        &base,
        "/v1/traces",
        Some("application/x-protobuf; charset=utf-8"),
        req.encode_to_vec(),
    )
    .await;
    assert_eq!(resp.status(), 200, "charset suffix");

    let req = common::trace_request(vec![common::span([12u8; 16], [1u8; 8], "bare", t0)]);
    let resp = post(
        &client,
        &base,
        "/v1/traces",
        Some("application/protobuf"),
        req.encode_to_vec(),
    )
    .await;
    assert_eq!(resp.status(), 200, "application/protobuf");

    // missing content-type defers to body parsing
    let req = common::trace_request(vec![common::span([13u8; 16], [1u8; 8], "noct", t0)]);
    let resp = post(&client, &base, "/v1/traces", None, req.encode_to_vec()).await;
    assert_eq!(resp.status(), 200, "missing content-type");

    // corrupt gzip is a 400, not a panic
    let resp = post(
        &client,
        &base,
        "/v1/traces",
        Some(PB),
        vec![0x1f, 0x8b, 0xde, 0xad, 0xbe, 0xef],
    )
    .await;
    assert_eq!(resp.status(), 400);

    // gzip bytes without a content-encoding header are still decompressed
    let req = common::trace_request(vec![common::span([51u8; 16], [1u8; 8], "sniffed", t0)]);
    let resp = post(
        &client,
        &base,
        "/v1/traces",
        Some(PB),
        common::gzip(&req.encode_to_vec()),
    )
    .await;
    assert_eq!(resp.status(), 200, "gzip sniffed from magic bytes");

    // declared gzip with a non-gzip body is rejected
    let plain = common::trace_request(vec![common::span([52u8; 16], [1u8; 8], "plain", t0)]);
    let resp = post(
        &client,
        &base,
        "/v1/traces",
        Some(PB),
        plain.encode_to_vec(),
    )
    .await;
    assert_eq!(resp.status(), 200, "plain body still accepted");
    let resp = client
        .post(format!("{base}/v1/traces"))
        .header(reqwest::header::CONTENT_TYPE, PB)
        .header(reqwest::header::CONTENT_ENCODING, "gzip")
        .body(vec![0x00, 0x01, 0x02])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400, "declared gzip must decompress");

    // oversized body hits the 64MB limit: server either answers 413 or aborts
    // the connection mid-body; both are rejections
    let oversized = client
        .post(format!("{base}/v1/traces"))
        .header(reqwest::header::CONTENT_TYPE, PB)
        .body(vec![0u8; 65 * 1024 * 1024])
        .send()
        .await;
    match oversized {
        Ok(resp) => assert_eq!(resp.status(), 413),
        Err(e) => assert!(e.is_connect() || e.is_body() || e.is_request()),
    }

    // unsupported metric types answer 200 and store nothing
    let hist = opentelemetry_proto::tonic::metrics::v1::Metric {
        name: "histo".into(),
        data: Some(
            opentelemetry_proto::tonic::metrics::v1::metric::Data::Histogram(
                opentelemetry_proto::tonic::metrics::v1::Histogram::default(),
            ),
        ),
        ..Default::default()
    };
    let resp = post(
        &client,
        &base,
        "/v1/metrics",
        Some(PB),
        common::metrics_request(vec![hist]).encode_to_vec(),
    )
    .await;
    assert_eq!(resp.status(), 200);

    // one request, two resources → two resource rows, both spans stored
    let mk_span = |trace: u8| common::span([trace; 16], [trace; 8], "multi", t0);
    let rs_a = opentelemetry_proto::tonic::trace::v1::ResourceSpans {
        resource: common::rust_resource(),
        scope_spans: vec![opentelemetry_proto::tonic::trace::v1::ScopeSpans {
            scope: common::rust_scope(),
            spans: vec![mk_span(21)],
            ..Default::default()
        }],
        ..Default::default()
    };
    let rs_b = opentelemetry_proto::tonic::trace::v1::ResourceSpans {
        resource: Some(opentelemetry_proto::tonic::resource::v1::Resource {
            attributes: vec![common::kv("service.name", "other-service")],
            ..Default::default()
        }),
        scope_spans: vec![opentelemetry_proto::tonic::trace::v1::ScopeSpans {
            scope: common::rust_scope(),
            spans: vec![mk_span(22)],
            ..Default::default()
        }],
        ..Default::default()
    };
    let req = opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest {
        resource_spans: vec![rs_a, rs_b],
    };
    let resp = post(&client, &base, "/v1/traces", Some(PB), req.encode_to_vec()).await;
    assert_eq!(resp.status(), 200);

    // duplicate export of the same request: idempotent
    let dup = common::trace_request(vec![common::span([31u8; 16], [1u8; 8], "dup", t0)]);
    let dup_bytes = dup.encode_to_vec();
    let resp = post(&client, &base, "/v1/traces", Some(PB), dup_bytes.clone()).await;
    assert_eq!(resp.status(), 200);
    let resp = post(&client, &base, "/v1/traces", Some(PB), dup_bytes).await;
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(Duration::from_millis(1500)).await;
    let status = common::terminate(&mut child);
    assert!(status.success());

    let db = common::open_read_only(&data_dir);
    // charset + bare + no-ct + sniffed + plain + multi-resource(2) + dup = 8 span rows
    assert_eq!(common::cf_count(&db, "spans_by_trace"), 8);
    assert_eq!(
        common::cf_count(&db, "resources"),
        2,
        "one row per distinct resource"
    );
    assert_eq!(common::cf_count(&db, "points"), 0, "histogram not stored");
}

#[tokio::test]
async fn flushed_data_survives_a_sigkill() {
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("microtel-kill-")
        .tempdir_in(&repo_tmp)
        .unwrap();
    let data_dir = dir.path().join("data");
    let (mut child, addr) = common::spawn_server(&data_dir, None);
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();
    let req = common::trace_request(vec![common::span(
        [41u8; 16],
        [1u8; 8],
        "survivor",
        common::now_ns(),
    )]);
    let resp = post(&client, &base, "/v1/traces", Some(PB), req.encode_to_vec()).await;
    assert_eq!(resp.status(), 200);

    // past the 1s flush interval: the batch is in rocksdb before the kill
    tokio::time::sleep(Duration::from_millis(1500)).await;
    unsafe {
        libc::kill(child.id() as libc::c_int, libc::SIGKILL);
    }
    child.wait().unwrap();

    let db = common::open_read_only(&data_dir);
    assert_eq!(common::cf_count(&db, "spans_by_trace"), 1);
    assert_eq!(common::cf_count(&db, "spans_by_time"), 1);
}

#[tokio::test]
async fn backpressure_429_when_the_queue_is_saturated() {
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("microtel-429-")
        .tempdir_in(&repo_tmp)
        .unwrap();
    let data_dir = dir.path().join("data");
    let cfg_path = dir.path().join("microtel.yaml");
    // capacity 4, watermark above capacity, interval long: the queue saturates deterministically
    std::fs::write(
        &cfg_path,
        "listen: 127.0.0.1:0\nqueue:\n  capacity: 4\n  watermark: 100\n  flush_ms: 2000\n",
    )
    .unwrap();
    let (mut child, addr) = common::spawn_server(&data_dir, Some(&cfg_path));
    let base = format!("http://{addr}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();

    let mut accepted = 0;
    let mut rejected = 0;
    for i in 0..12u8 {
        let req = common::trace_request(vec![common::span(
            [i + 1; 16],
            [1u8; 8],
            "flood",
            common::now_ns(),
        )]);
        let resp = post(&client, &base, "/v1/traces", Some(PB), req.encode_to_vec()).await;
        match resp.status() {
            reqwest::StatusCode::OK => accepted += 1,
            reqwest::StatusCode::TOO_MANY_REQUESTS => rejected += 1,
            other => panic!("unexpected status {other} on flood request {i}"),
        }
    }
    assert!(accepted >= 4, "capacity worth of requests accepted");
    assert!(rejected > 0, "saturated queue must 429");

    // metrics have their own queue: a saturates traces queue must not reject them
    let metrics = common::metrics_request(vec![common::gauge_metric(
        "independent",
        vec![common::kv("env", "test")],
        common::now_ns(),
    )]);
    let resp = post(
        &client,
        &base,
        "/v1/metrics",
        Some(PB),
        metrics.encode_to_vec(),
    )
    .await;
    assert_eq!(resp.status(), 200, "signals have independent queues");

    // after the flush interval the queue drains and accepts again
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let req = common::trace_request(vec![common::span(
        [60u8; 16],
        [1u8; 8],
        "post-drain",
        common::now_ns(),
    )]);
    let resp = post(&client, &base, "/v1/traces", Some(PB), req.encode_to_vec()).await;
    assert_eq!(resp.status(), 200, "queue accepts again after draining");

    let status = common::terminate(&mut child);
    assert!(status.success());
}
