#![allow(dead_code)]

use std::io::{BufRead, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use microtel::keys;
use microtel::store::{MetricBatch, PointRow, SeriesRow, SpanBatch, SpanRow};
use opentelemetry_proto::tonic::collector::metrics::v1::ExportMetricsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use opentelemetry_proto::tonic::common::v1::{AnyValue, InstrumentationScope, KeyValue};
use opentelemetry_proto::tonic::metrics::v1 as metrics;
use opentelemetry_proto::tonic::metrics::v1::{Gauge, Metric, NumberDataPoint, Sum};
use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::trace::v1::{ResourceSpans, ScopeSpans, Span};
use prost::Message;
use rocksdb::{ColumnFamilyDescriptor, DB, IteratorMode, Options};

pub use microtel::proto::SpanEnvelope;

pub fn kv(key: &str, value: &str) -> KeyValue {
    KeyValue {
        key: key.into(),
        value: Some(AnyValue {
            value: Some(
                opentelemetry_proto::tonic::common::v1::any_value::Value::StringValue(value.into()),
            ),
        }),
        ..Default::default()
    }
}

pub fn now_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

pub fn span(trace_id: [u8; 16], span_id: [u8; 8], name: &str, start_ns: u64) -> Span {
    Span {
        trace_id: trace_id.to_vec(),
        span_id: span_id.to_vec(),
        name: name.into(),
        start_time_unix_nano: start_ns,
        end_time_unix_nano: start_ns + 1_000_000,
        ..Default::default()
    }
}

pub fn rust_resource() -> Option<Resource> {
    Some(Resource {
        attributes: vec![kv("service.name", "rust-test")],
        ..Default::default()
    })
}

pub fn rust_scope() -> Option<InstrumentationScope> {
    Some(InstrumentationScope {
        name: "rust-test-scope".into(),
        ..Default::default()
    })
}

pub fn trace_request(spans: Vec<Span>) -> ExportTraceServiceRequest {
    ExportTraceServiceRequest {
        resource_spans: vec![ResourceSpans {
            resource: rust_resource(),
            scope_spans: vec![ScopeSpans {
                scope: rust_scope(),
                spans,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

pub fn gauge_metric(name: &str, attrs: Vec<KeyValue>, time_ns: u64) -> Metric {
    Metric {
        name: name.into(),
        data: Some(metrics::metric::Data::Gauge(Gauge {
            data_points: vec![NumberDataPoint {
                attributes: attrs,
                time_unix_nano: time_ns,
                value: Some(metrics::number_data_point::Value::AsDouble(42.0)),
                ..Default::default()
            }],
        })),
        ..Default::default()
    }
}

pub fn sum_metric(name: &str, time_ns: u64) -> Metric {
    Metric {
        name: name.into(),
        data: Some(metrics::metric::Data::Sum(Sum {
            data_points: vec![NumberDataPoint {
                time_unix_nano: time_ns,
                value: Some(metrics::number_data_point::Value::AsInt(7)),
                ..Default::default()
            }],
            ..Default::default()
        })),
        ..Default::default()
    }
}

pub fn metrics_request(metrics: Vec<Metric>) -> ExportMetricsServiceRequest {
    ExportMetricsServiceRequest {
        resource_metrics: vec![opentelemetry_proto::tonic::metrics::v1::ResourceMetrics {
            resource: rust_resource(),
            scope_metrics: vec![opentelemetry_proto::tonic::metrics::v1::ScopeMetrics {
                scope: rust_scope(),
                metrics,
                ..Default::default()
            }],
            ..Default::default()
        }],
    }
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = flate2::write::GzEncoder::new(&mut out, flate2::Compression::default());
    enc.write_all(data).unwrap();
    enc.finish().unwrap();
    out
}

pub fn spawn_server(data_dir: &Path, config: Option<&Path>) -> (Child, String) {
    let exe = env!("CARGO_BIN_EXE_microtel");
    let mut cmd = Command::new(exe);
    cmd.arg("--storage-dir").arg(data_dir.to_str().unwrap());
    if let Some(cfg) = config {
        cmd.arg("--config").arg(cfg.to_str().unwrap());
    } else {
        cmd.arg("--listen").arg("127.0.0.1:0");
    }
    let mut child = cmd
        .stdout(Stdio::inherit())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn microtel");
    let stderr = child.stderr.take().expect("piped stderr");
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut reader = std::io::BufReader::new(stderr);
    let mut addr: Option<String> = None;
    let mut seen = String::new();
    let mut line = String::new();
    loop {
        line.clear();
        if std::io::BufRead::read_line(&mut reader, &mut line).unwrap_or(0) == 0 {
            break;
        }
        seen.push_str(&line);
        if let Some(a) = line.split("listening on ").nth(1) {
            addr = Some(a.trim().to_string());
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
    }
    // keep draining the child's stderr so it never blocks on a full pipe
    std::thread::spawn(move || {
        for line in reader.lines().map_while(Result::ok) {
            eprintln!("{line}");
        }
    });
    let Some(addr) = addr else {
        let _ = child.wait();
        panic!("server did not report its address; stderr:\n{seen}");
    };
    (child, addr)
}

pub fn terminate(child: &mut Child) -> std::process::ExitStatus {
    // libc::kill is FFI; std only offers SIGKILL (Child::kill).
    #[allow(unsafe_code)]
    unsafe {
        libc::kill(child.id() as libc::c_int, libc::SIGTERM);
    }
    child.wait().expect("wait after SIGTERM")
}

pub fn run_emitter(addr: &str, a_traces: usize, b_traces: usize) {
    let tools = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools");
    if !tools.join("node_modules").exists() {
        let status = Command::new("pnpm")
            .args(["install", "--frozen-lockfile"])
            .current_dir(&tools)
            .status()
            .expect("pnpm available; run `pnpm install` in tools/ if missing");
        assert!(status.success(), "pnpm install failed");
    }
    let status = Command::new("node")
        .arg("emit.mjs")
        .current_dir(&tools)
        .env("EMIT_ENDPOINT", format!("http://{addr}"))
        .env("EMIT_A_TRACES", a_traces.to_string())
        .env("EMIT_B_TRACES", b_traces.to_string())
        .status()
        .expect("node available");
    assert!(status.success(), "node emitter failed: {status}");
}

/// UTC calendar date of `ns`, for purge cutoffs.
pub fn date_of(ns: u64) -> time::Date {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ns))
        .expect("in range")
        .date()
}

pub fn cf_count(db: &DB, cf: &str) -> usize {
    let handle = db.cf_handle(cf).expect("cf exists");
    let mut rows = 0;
    for kv in db.iterator_cf(handle, IteratorMode::Start) {
        kv.expect("cf iterates");
        rows += 1;
    }
    rows
}

/// Read-only inspection handle. Every column family needs the merge operator:
/// `DB::open_cf_for_read_only` would give each CF `Options::default()` and the
/// unresolved `spans_by_time` operands would read as missing rows.
pub fn open_read_only(data_dir: &Path) -> DB {
    let names = [
        "default",
        "spans_by_trace",
        "spans_by_time",
        "resources",
        "scopes",
        "series",
        "points",
    ];
    let cfs: Vec<_> = names
        .iter()
        .map(|name| {
            let mut opts = Options::default();
            opts.set_merge_operator_associative("span_id_union", microtel::store::union_span_ids);
            ColumnFamilyDescriptor::new(*name, opts)
        })
        .collect();
    DB::open_cf_descriptors_read_only(&Options::default(), data_dir, cfs, false)
        .expect("read-only open")
}

// --- storage model builders (mirror ingest.rs construction) ---

pub fn span_row(trace_id: u8, span_id: u8, name: &str, start_ns: u64) -> SpanRow {
    SpanRow {
        ymd: keys::ymd(start_ns),
        trace_id: vec![trace_id; 16],
        span_id: vec![span_id; 8],
        start_ns,
        envelope: SpanEnvelope {
            resource_hash: vec![0xAA; 16],
            scope_hash: vec![0xBB; 16],
            span: Some(span([trace_id; 16], [span_id; 8], name, start_ns)),
        }
        .encode_to_vec(),
    }
}

pub fn span_batch(spans: Vec<SpanRow>) -> SpanBatch {
    SpanBatch {
        resources: Vec::new(),
        scopes: Vec::new(),
        spans,
    }
}

pub fn point_row(name: &str, attr_hash: [u8; 16], time_ns: u64) -> PointRow {
    PointRow {
        name: name.into(),
        attr_hash,
        ymd: keys::ymd(time_ns),
        time_ns,
        value: Vec::new(),
    }
}

pub fn series_row(name: &str, attr_hash: [u8; 16]) -> SeriesRow {
    SeriesRow {
        name: name.into(),
        attr_hash,
        value: microtel::proto::AttrSet {
            attributes: vec![kv("k", "v")],
            kind: 0,
        }
        .encode_to_vec(),
    }
}

pub fn metric_batch(series: Vec<SeriesRow>, points: Vec<PointRow>) -> MetricBatch {
    MetricBatch { series, points }
}
