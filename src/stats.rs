//! In-memory ingestion stats, rendered Prometheus-style on `GET /stats`.
//! Process-local on purpose: restarts reset everything.

use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crate::store::Store;

#[derive(Clone, Copy)]
pub enum Endpoint {
    Traces,
    Metrics,
    Logs,
}

#[derive(Clone, Copy)]
pub enum Signal {
    Traces,
    Metrics,
}

const STATUSES: [u16; 7] = [200, 400, 413, 415, 429, 501, 503];
const ENDPOINTS: [(&str, usize); 3] = [("traces", 0), ("metrics", 1), ("logs", 2)];
const SIGNALS: [(&str, usize); 2] = [("traces", 0), ("metrics", 1)];

pub struct Stats {
    started: Instant,
    requests: [[AtomicU64; STATUSES.len()]; ENDPOINTS.len()],
    spans_received: AtomicU64,
    spans_stored: AtomicU64,
    spans_dropped: AtomicU64,
    points_received: AtomicU64,
    points_stored: AtomicU64,
    histograms_received: AtomicU64,
    histograms_stored: AtomicU64,
    queue_depth: [Arc<AtomicU64>; 2],
}

impl Stats {
    pub fn new() -> Stats {
        Stats {
            started: Instant::now(),
            requests: std::array::from_fn(|_| std::array::from_fn(|_| AtomicU64::new(0))),
            spans_received: AtomicU64::new(0),
            spans_stored: AtomicU64::new(0),
            spans_dropped: AtomicU64::new(0),
            points_received: AtomicU64::new(0),
            points_stored: AtomicU64::new(0),
            histograms_received: AtomicU64::new(0),
            histograms_stored: AtomicU64::new(0),
            queue_depth: [Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0))],
        }
    }

    /// Depth handle for one signal's writer queue; rendered as `queue_depth`.
    pub fn gauge(&self, signal: Signal) -> Arc<AtomicU64> {
        self.queue_depth[signal as usize].clone()
    }

    /// Unknown statuses are ignored; every ingest response path is listed in `STATUSES`.
    pub fn request(&self, endpoint: Endpoint, status: u16) {
        if let Some(s) = STATUSES.iter().position(|&known| known == status) {
            self.requests[endpoint as usize][s].fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn spans_received(&self, n: u64) {
        self.spans_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn spans_stored(&self, n: u64) {
        self.spans_stored.fetch_add(n, Ordering::Relaxed);
    }

    pub fn spans_dropped(&self, n: u64) {
        self.spans_dropped.fetch_add(n, Ordering::Relaxed);
    }

    pub fn points_received(&self, n: u64) {
        self.points_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn points_stored(&self, n: u64) {
        self.points_stored.fetch_add(n, Ordering::Relaxed);
    }

    pub fn histograms_received(&self, n: u64) {
        self.histograms_received.fetch_add(n, Ordering::Relaxed);
    }

    pub fn histograms_stored(&self, n: u64) {
        self.histograms_stored.fetch_add(n, Ordering::Relaxed);
    }

    pub fn render(&self, store: &Store) -> String {
        let mut out = String::with_capacity(2048);
        let val = |a: &AtomicU64| a.load(Ordering::Relaxed);

        let _ = writeln!(
            out,
            "# HELP microtel_ingest_requests_total Ingest requests by endpoint and response status."
        );
        let _ = writeln!(out, "# TYPE microtel_ingest_requests_total counter");
        for (label, i) in ENDPOINTS {
            for (s, status) in STATUSES.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "microtel_ingest_requests_total{{endpoint=\"{label}\",status=\"{status}\"}} {}",
                    val(&self.requests[i][s])
                );
            }
        }

        counter(
            &mut out,
            "spans_received_total",
            "Spans accepted into the ingest queue.",
            val(&self.spans_received),
        );
        counter(
            &mut out,
            "spans_stored_total",
            "Spans persisted to RocksDB.",
            val(&self.spans_stored),
        );
        counter(
            &mut out,
            "spans_dropped_total",
            "Spans not stored: invalid ids or lost after failed flushes.",
            val(&self.spans_dropped),
        );
        counter(
            &mut out,
            "points_received_total",
            "Metric datapoints (gauge/sum) accepted into the ingest queue.",
            val(&self.points_received),
        );
        counter(
            &mut out,
            "points_stored_total",
            "Metric datapoints persisted to RocksDB.",
            val(&self.points_stored),
        );
        counter(
            &mut out,
            "histograms_received_total",
            "Histogram datapoints accepted into the ingest queue.",
            val(&self.histograms_received),
        );
        counter(
            &mut out,
            "histograms_stored_total",
            "Histogram datapoints persisted to RocksDB.",
            val(&self.histograms_stored),
        );

        for (label, i) in SIGNALS {
            gauge(
                &mut out,
                "queue_depth",
                "Batches buffered in a signal's writer queue.",
                &format!("signal=\"{label}\""),
                val(&self.queue_depth[i]) as f64,
            );
        }
        for (cf, rows) in store.estimate_rows() {
            gauge(
                &mut out,
                "db_rows",
                "Estimated rows per column family.",
                &format!("cf=\"{cf}\""),
                rows as f64,
            );
        }
        gauge(
            &mut out,
            "uptime_seconds",
            "Seconds since process start.",
            "",
            self.started.elapsed().as_secs_f64(),
        );
        out
    }
}

impl Default for Stats {
    fn default() -> Self {
        Self::new()
    }
}

fn counter(out: &mut String, name: &str, help: &str, v: u64) {
    let _ = writeln!(out, "# HELP microtel_{name} {help}");
    let _ = writeln!(out, "# TYPE microtel_{name} counter");
    let _ = writeln!(out, "microtel_{name} {v}");
}

fn gauge(out: &mut String, name: &str, help: &str, labels: &str, v: f64) {
    let _ = writeln!(out, "# HELP microtel_{name} {help}");
    let _ = writeln!(out, "# TYPE microtel_{name} gauge");
    if labels.is_empty() {
        let _ = writeln!(out, "microtel_{name} {v}");
    } else {
        let _ = writeln!(out, "microtel_{name}{{{labels}}} {v}");
    }
}
