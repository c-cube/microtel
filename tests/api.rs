mod common;

use common::*;
use microtel::keys;
use microtel::proto::{AttrSet, MetricKind, NumberDataPoint, Resource, SpanEnvelope, hash16};
use microtel::query::{self, GroupBy};
use microtel::store::{MetricBatch, PointRow, SeriesRow, SpanBatch, SpanRow, Store};
use opentelemetry_proto::tonic::common::v1::KeyValue;
use opentelemetry_proto::tonic::metrics::v1 as otel_metrics;
use opentelemetry_proto::tonic::trace::v1::Span;
use prost::Message;

const BASE_MS: u64 = 1_750_000_000_000;
const RANGE_MS: u64 = 6 * 3_600_000; // 2-minute buckets for this range

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::open(dir.path()).expect("open");
    (dir, store)
}

fn res(name: &str) -> ([u8; 16], Resource) {
    let r = Resource {
        attributes: vec![kv("service.name", name)],
        ..Default::default()
    };
    (hash16(&r), r)
}

fn span_row(res_hash: [u8; 16], trace: u8, span: u8, name: &str, start_ns: u64) -> SpanRow {
    SpanRow {
        ymd: keys::ymd(start_ns),
        trace_id: vec![trace; 16],
        span_id: vec![span; 8],
        start_ns,
        envelope: SpanEnvelope {
            resource_hash: res_hash.to_vec(),
            scope_hash: vec![0xBB; 16],
            span: Some(Span {
                trace_id: vec![trace; 16],
                span_id: vec![span; 8],
                name: name.into(),
                start_time_unix_nano: start_ns,
                end_time_unix_nano: start_ns + 1_000_000,
                ..Default::default()
            }),
        }
        .encode_to_vec(),
    }
}

fn batch(resources: Vec<([u8; 16], Resource)>, spans: Vec<SpanRow>) -> SpanBatch {
    SpanBatch {
        resources: resources
            .into_iter()
            .map(|(h, r)| (h, r.encode_to_vec()))
            .collect(),
        scopes: Vec::new(),
        spans,
    }
}

fn ns(base_ms: u64, offset_ms: u64) -> u64 {
    (base_ms + offset_ms) * 1_000_000
}

fn dp(time_ns: u64, v: otel_metrics::number_data_point::Value) -> Vec<u8> {
    #[allow(unused_imports)]
    use prost::Message as _;
    NumberDataPoint {
        time_unix_nano: time_ns,
        value: Some(v),
        ..Default::default()
    }
    .encode_to_vec()
}

fn attr_hash(name: &str) -> [u8; 16] {
    hash16(&AttrSet {
        attributes: vec![kv("service.name", name)],
        kind: MetricKind::Unspecified as i32,
    })
}

fn series_row(name: &str, svc: &str, kind: MetricKind, hash: [u8; 16]) -> SeriesRow {
    SeriesRow {
        name: name.into(),
        attr_hash: hash,
        value: AttrSet {
            attributes: vec![kv("service.name", svc)],
            kind: kind as i32,
        }
        .encode_to_vec(),
    }
}

fn bucket_total(chart: &query::Barchart, label: &str) -> Vec<u64> {
    let i = chart.labels.iter().position(|l| l == label).expect("label");
    chart.buckets[1 + i].clone()
}

#[test]
fn barchart_counts_groups_by_service() {
    let (_dir, store) = store();
    let (ha, ra) = res("svc-a");
    let (hb, rb) = res("svc-b");
    store
        .apply_traces(&batch(
            vec![(ha, ra), (hb, rb)],
            vec![
                span_row(ha, 1, 1, "op", ns(BASE_MS, 0)),
                span_row(ha, 2, 2, "op", ns(BASE_MS, 120_000)),
                span_row(ha, 3, 3, "op", ns(BASE_MS, 240_000)),
                span_row(hb, 4, 4, "op", ns(BASE_MS, 120_000)),
            ],
        ))
        .unwrap();

    let chart = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::Service,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(chart.bucket_ms, 120_000);
    assert!(!chart.truncated);
    assert_eq!(chart.labels, vec!["svc-a", "svc-b"]);
    assert_eq!(bucket_total(&chart, "svc-a"), {
        let mut v = vec![0u64; 180];
        v[0] = 1;
        v[1] = 1;
        v[2] = 1;
        v
    });
    assert_eq!(bucket_total(&chart, "svc-b")[1], 1);
    assert_eq!(chart.buckets[0][1], BASE_MS + 120_000);
}

#[test]
fn barchart_group_by_none_and_name() {
    let (_dir, store) = store();
    let (h, r) = res("svc-a");
    store
        .apply_traces(&batch(
            vec![(h, r.clone())],
            vec![
                span_row(h, 1, 1, "op-a", ns(BASE_MS, 0)),
                span_row(h, 2, 2, "op-b", ns(BASE_MS, 120_000)),
            ],
        ))
        .unwrap();

    let none = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::None,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(none.labels, vec!["spans"]);
    assert_eq!(bucket_total(&none, "spans")[0], 1);
    assert_eq!(bucket_total(&none, "spans")[1], 1);

    let by_name = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::Name,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(by_name.labels, vec!["op-a", "op-b"]);
}

#[test]
fn barchart_filters_by_selected_services() {
    let (_dir, store) = store();
    let (ha, ra) = res("svc-a");
    let (hb, rb) = res("svc-b");
    store
        .apply_traces(&batch(
            vec![(ha, ra), (hb, rb)],
            vec![
                span_row(ha, 1, 1, "op", ns(BASE_MS, 0)),
                span_row(hb, 2, 2, "op", ns(BASE_MS, 0)),
            ],
        ))
        .unwrap();

    let chart = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::Service,
        &["svc-b".to_string()],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(chart.labels, vec!["svc-b"]);
    assert_eq!(bucket_total(&chart, "svc-b")[0], 1);
}

#[test]
fn barchart_exemplars_are_first_seen_per_bucket_and_group() {
    let (_dir, store) = store();
    let (h, r) = res("svc-a");
    store
        .apply_traces(&batch(
            vec![(h, r)],
            vec![
                span_row(h, 0x01, 0x11, "op", ns(BASE_MS, 0)),
                span_row(h, 0x02, 0x12, "op", ns(BASE_MS, 120_000)),
            ],
        ))
        .unwrap();

    let chart = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::Service,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(chart.exemplars.len(), 2);
    let first = &chart.exemplars[0];
    assert_eq!(first.t, BASE_MS);
    assert_eq!(first.trace_id, "01".repeat(16));
    assert_eq!(first.span_id, "11".repeat(8));
    assert_eq!(chart.exemplars[1].trace_id, "02".repeat(16));
    assert_eq!(chart.exemplars[1].t, BASE_MS + 120_000);
}

#[test]
fn barchart_folds_tail_groups_into_other() {
    let (_dir, store) = store();
    let mut resources = Vec::new();
    let mut spans = Vec::new();
    for i in 0..13u8 {
        let (h, r) = res(&format!("svc-{i}"));
        resources.push((h, r));
        spans.push(span_row(h, i, i, "op", ns(BASE_MS, 0)));
    }
    store.apply_traces(&batch(resources, spans)).unwrap();

    let chart = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::Service,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(chart.labels.len(), 13);
    assert_eq!(chart.labels.last().unwrap(), "other");
    assert_eq!(bucket_total(&chart, "other")[0], 1);
    assert_eq!(
        chart
            .exemplars
            .iter()
            .filter(|e| e.label == "other")
            .count(),
        1
    );
}

#[test]
fn barchart_reports_truncation_at_cap() {
    let (_dir, store) = store();
    let (h, r) = res("svc-a");
    let spans: Vec<SpanRow> = (0..10u8)
        .map(|i| span_row(h, i, i, "op", ns(BASE_MS, 0)))
        .collect();
    store.apply_traces(&batch(vec![(h, r)], spans)).unwrap();

    let chart = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + RANGE_MS,
        GroupBy::Service,
        &[],
        5,
        None,
    );
    assert!(chart.truncated);
    let total: u64 = bucket_total(&chart, "svc-a").iter().sum();
    assert_eq!(total, 5);
}

#[test]
fn points_gauge_averages_and_sum_takes_last_per_bucket() {
    let (_dir, store) = store();
    let hg = attr_hash("svc-a");
    let hs = attr_hash("svc-a");
    store
        .apply_metrics(&MetricBatch {
            series: vec![
                series_row("g1", "svc-a", MetricKind::Gauge, hg),
                series_row("s1", "svc-a", MetricKind::Sum, hs),
            ],
            points: vec![
                PointRow {
                    name: "g1".into(),
                    attr_hash: hg,
                    ymd: keys::ymd(ns(BASE_MS, 0)),
                    time_ns: ns(BASE_MS, 0),
                    value: dp(
                        ns(BASE_MS, 0),
                        otel_metrics::number_data_point::Value::AsDouble(10.0),
                    ),
                },
                PointRow {
                    name: "g1".into(),
                    attr_hash: hg,
                    ymd: keys::ymd(ns(BASE_MS, 30_000)),
                    time_ns: ns(BASE_MS, 30_000),
                    value: dp(
                        ns(BASE_MS, 30_000),
                        otel_metrics::number_data_point::Value::AsDouble(20.0),
                    ),
                },
                PointRow {
                    name: "g1".into(),
                    attr_hash: hg,
                    ymd: keys::ymd(ns(BASE_MS, 120_000)),
                    time_ns: ns(BASE_MS, 120_000),
                    value: dp(
                        ns(BASE_MS, 120_000),
                        otel_metrics::number_data_point::Value::AsDouble(30.0),
                    ),
                },
                PointRow {
                    name: "s1".into(),
                    attr_hash: hs,
                    ymd: keys::ymd(ns(BASE_MS, 0)),
                    time_ns: ns(BASE_MS, 0),
                    value: dp(
                        ns(BASE_MS, 0),
                        otel_metrics::number_data_point::Value::AsInt(7),
                    ),
                },
                PointRow {
                    name: "s1".into(),
                    attr_hash: hs,
                    ymd: keys::ymd(ns(BASE_MS, 60_000)),
                    time_ns: ns(BASE_MS, 60_000),
                    value: dp(
                        ns(BASE_MS, 60_000),
                        otel_metrics::number_data_point::Value::AsInt(9),
                    ),
                },
            ],
        })
        .unwrap();

    let g = query::points(
        &store,
        "g1",
        BASE_MS,
        BASE_MS + RANGE_MS,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(g.lines.len(), 1);
    assert_eq!(g.lines[0].label, "service.name=svc-a");
    assert_eq!(g.lines[0].y[0], Some(15.0));
    assert_eq!(g.lines[0].y[1], Some(30.0));
    assert_eq!(g.lines[0].y[2], None);

    let s = query::points(
        &store,
        "s1",
        BASE_MS,
        BASE_MS + RANGE_MS,
        &[],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(s.lines[0].y[0], Some(9.0));
    assert_eq!(s.lines[0].y[1], None);
}

#[test]
fn points_filter_by_service_and_sort_lines_by_label() {
    let (_dir, store) = store();
    for (metric, svc) in [("m1", "zzz"), ("m1", "aaa")] {
        let h = attr_hash(svc);
        store
            .apply_metrics(&MetricBatch {
                series: vec![series_row(metric, svc, MetricKind::Gauge, h)],
                points: vec![PointRow {
                    name: metric.into(),
                    attr_hash: h,
                    ymd: keys::ymd(ns(BASE_MS, 0)),
                    time_ns: ns(BASE_MS, 0),
                    value: dp(
                        ns(BASE_MS, 0),
                        otel_metrics::number_data_point::Value::AsDouble(1.0),
                    ),
                }],
            })
            .unwrap();
    }

    let all = query::points(
        &store,
        "m1",
        BASE_MS,
        BASE_MS + RANGE_MS,
        &[],
        query::SCAN_CAP,
        None,
    );
    let labels: Vec<&str> = all.lines.iter().map(|l| l.label.as_str()).collect();
    assert_eq!(labels, vec!["service.name=aaa", "service.name=zzz"]);

    let filtered = query::points(
        &store,
        "m1",
        BASE_MS,
        BASE_MS + RANGE_MS,
        &["aaa".to_string()],
        query::SCAN_CAP,
        None,
    );
    assert_eq!(filtered.lines.len(), 1);
    assert_eq!(filtered.lines[0].label, "service.name=aaa");
}

#[test]
fn series_lists_name_and_attrs() {
    let (_dir, store) = store();
    let h = attr_hash("svc-a");
    store
        .apply_metrics(&MetricBatch {
            series: vec![series_row("g1", "svc-a", MetricKind::Gauge, h)],
            points: Vec::new(),
        })
        .unwrap();

    let list = query::series(&store);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "g1");
    assert_eq!(
        list[0].attrs.get("service.name").map(String::as_str),
        Some("svc-a")
    );
}

// --- end-to-end through the HTTP server ---

#[test]
fn api_serves_charts_and_validates_params() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut child, addr) = spawn_server(dir.path(), None);

    run_emitter(&addr, 2, 2);
    std::thread::sleep(std::time::Duration::from_millis(1500));

    let to = common::now_ns() / 1_000_000 + 60_000;
    let from = to - 3_600_000;

    let series: serde_json::Value = ureq::get(&format!("http://{addr}/api/metrics/series"))
        .call()
        .expect("series 200")
        .into_json()
        .expect("series json");
    assert!(!series.as_array().expect("array").is_empty());

    let chart: serde_json::Value = ureq::get(&format!(
        "http://{addr}/api/spans/barchart?from={from}&to={to}&group_by=service"
    ))
    .call()
    .expect("barchart 200")
    .into_json()
    .expect("barchart json");
    assert_eq!(chart["bucket_ms"].as_u64().unwrap(), 15_000);
    assert!(
        chart["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .skip(1)
            .any(|row| row
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c.as_u64().unwrap() > 0))
    );
    let labels: Vec<&str> = chart["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(labels.contains(&"emitter-a"));

    let status_of = |url: String| match ureq::get(&url).call() {
        Ok(r) => r.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("transport error: {e}"),
    };
    assert_eq!(
        status_of(format!(
            "http://{addr}/api/spans/barchart?from={to}&to={from}"
        )),
        400
    );
    assert_eq!(
        status_of(format!(
            "http://{addr}/api/spans/barchart?from={from}&to={to}&buckets=nope"
        )),
        400
    );
    assert_eq!(
        status_of(format!(
            "http://{addr}/api/metrics/points?name=x&from={from}&to={to}&buckets=nope"
        )),
        400
    );
    assert_eq!(status_of(format!("http://{addr}/api/nope")), 404);

    // the SPA fallback answers every client route with the embedded index
    let root = ureq::get(&format!("http://{addr}/")).call().unwrap();
    assert_eq!(root.status(), 200);
    assert!(
        root.header("content-type")
            .unwrap_or("")
            .starts_with("text/html")
    );

    common::terminate(&mut child);
}

#[test]
fn pick_bucket_steps_are_sane() {
    assert_eq!(query::pick_bucket_ms(180_000), 1_000);
    assert_eq!(query::pick_bucket_ms(21_600_000), 120_000);
    assert_eq!(query::pick_bucket_ms(35 * 86_400_000), 10_800_000);
}

// --- trace lookup ---

fn hex_id(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[test]
fn trace_lookup_flat_sorted_with_service() {
    let (_dir, store) = store();
    let (h, r) = res("svc-a");
    let trace = [7u8; 16];
    let root = [1u8; 8];
    let child = [2u8; 8];

    let env = |span: [u8; 8],
               parent: Option<[u8; 8]>,
               name: &str,
               start_ns: u64,
               attrs: Vec<KeyValue>,
               code: i32| {
        SpanEnvelope {
            resource_hash: h.to_vec(),
            scope_hash: vec![0xBB; 16],
            span: Some(Span {
                trace_id: trace.to_vec(),
                span_id: span.to_vec(),
                parent_span_id: parent.map(|p| p.to_vec()).unwrap_or_default(),
                name: name.into(),
                start_time_unix_nano: start_ns,
                end_time_unix_nano: start_ns + 5_000_000,
                attributes: attrs,
                kind: 3,
                status: (code != 0).then(|| opentelemetry_proto::tonic::trace::v1::Status {
                    code,
                    message: "card_declined".into(),
                }),
                ..Default::default()
            }),
        }
    };

    let rows = vec![
        SpanRow {
            ymd: keys::ymd(ns(BASE_MS, 0)),
            trace_id: trace.to_vec(),
            span_id: root.to_vec(),
            start_ns: ns(BASE_MS, 0),
            envelope: env(
                root,
                None,
                "root-op",
                ns(BASE_MS, 0),
                vec![kv("http.method", "GET")],
                1,
            )
            .encode_to_vec(),
        },
        SpanRow {
            ymd: keys::ymd(ns(BASE_MS, 10)),
            trace_id: trace.to_vec(),
            span_id: child.to_vec(),
            start_ns: ns(BASE_MS, 10),
            envelope: env(
                child,
                Some(root),
                "child-op",
                ns(BASE_MS, 10),
                Vec::new(),
                2,
            )
            .encode_to_vec(),
        },
    ];
    store.apply_traces(&batch(vec![(h, r)], rows)).unwrap();

    let tid = hex_id(&trace);
    let spans = query::trace(&store, &tid, BASE_MS).unwrap();
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].span_id, hex_id(&root));
    assert_eq!(spans[0].parent_span_id, None);
    assert_eq!(spans[0].name, "root-op");
    assert_eq!(spans[0].service, "svc-a");
    assert_eq!(spans[0].duration_ns, 5_000_000);
    assert!(!spans[0].error);
    assert_eq!(spans[0].status_message, "card_declined");
    assert_eq!(spans[0].kind, 3);
    assert!(spans[1].error);
    assert_eq!(
        spans[0].attrs.get("http.method").map(String::as_str),
        Some("GET")
    );
    assert_eq!(
        spans[1].parent_span_id.as_deref(),
        Some(hex_id(&root).as_str())
    );

    assert!(matches!(
        query::trace(&store, "zz", BASE_MS),
        Err(query::TraceError::BadId)
    ));
    assert!(matches!(
        query::trace(&store, &"0".repeat(32), BASE_MS),
        Err(query::TraceError::NotFound)
    ));
}

#[test]
fn trace_http_statuses() {
    let dir = tempfile::tempdir().expect("tempdir");
    {
        let store = Store::open(dir.path()).expect("open");
        let (h, r) = res("svc-a");
        let trace = [9u8; 16];
        let span = [5u8; 8];
        let now = common::now_ns();
        store
            .apply_traces(&batch(
                vec![(h, r)],
                vec![SpanRow {
                    ymd: keys::ymd(now),
                    trace_id: trace.to_vec(),
                    span_id: span.to_vec(),
                    start_ns: now,
                    envelope: SpanEnvelope {
                        resource_hash: h.to_vec(),
                        scope_hash: vec![0xBB; 16],
                        span: Some(Span {
                            trace_id: trace.to_vec(),
                            span_id: span.to_vec(),
                            name: "op".into(),
                            start_time_unix_nano: now,
                            end_time_unix_nano: now + 1_000_000,
                            ..Default::default()
                        }),
                    }
                    .encode_to_vec(),
                }],
            ))
            .unwrap();
    }
    let (mut child, addr) = spawn_server(dir.path(), None);
    let at = common::now_ns() / 1_000_000;
    let id = hex_id(&[9u8; 16]);

    let spans: serde_json::Value = ureq::get(&format!("http://{addr}/api/traces/{id}?at={at}"))
        .call()
        .expect("trace 200")
        .into_json()
        .expect("trace json");
    assert_eq!(spans.as_array().expect("array").len(), 1);
    assert_eq!(spans[0]["name"], "op");
    assert_eq!(spans[0]["service"], "svc-a");

    let code = match ureq::get(&format!("http://{addr}/api/traces/{}", "0".repeat(32))).call() {
        Ok(_) => panic!("unknown trace must 404"),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("transport error: {e}"),
    };
    assert_eq!(code, 404);

    let code = match ureq::get(&format!("http://{addr}/api/traces/zz")).call() {
        Ok(_) => panic!("bad hex must 400"),
        Err(ureq::Error::Status(code, _)) => code,
        Err(e) => panic!("transport error: {e}"),
    };
    assert_eq!(code, 400);

    common::terminate(&mut child);
}

#[test]
fn bucket_count_is_client_driven_and_capped() {
    let (_dir, store) = store();

    let chart = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + 3_600_000,
        GroupBy::Service,
        &[],
        query::SCAN_CAP,
        Some(20),
    );
    // 1h / 20 -> a snapped 3m step, so exactly 20 buckets
    assert_eq!(chart.bucket_ms, 180_000);
    assert_eq!(chart.buckets[0].len(), 20);

    let fine = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + 3_600_000,
        GroupBy::Service,
        &[],
        query::SCAN_CAP,
        Some(100_000),
    );
    assert!(fine.buckets[0].len() <= query::MAX_BUCKETS);
    assert_eq!(fine.bucket_ms, query::pick_bucket_ms(3_600_000));

    let coarse = query::barchart(
        &store,
        BASE_MS,
        BASE_MS + 3_600_000,
        GroupBy::Service,
        &[],
        query::SCAN_CAP,
        Some(1),
    );
    assert!(coarse.buckets[0].len() <= 12);
}
