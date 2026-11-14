mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use microtel::config::Queue;
use microtel::pipeline::{self, Pipeline};
use microtel::store::SpanBatch;
use microtel::store::Store;
use tokio::sync::mpsc;

fn tmp_dir(tag: &str) -> tempfile::TempDir {
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    tempfile::Builder::new()
        .prefix(&format!("microtel-pipe-{tag}-"))
        .tempdir_in(&repo_tmp)
        .unwrap()
}

fn span_batch(i: u8) -> SpanBatch {
    common::span_batch(vec![common::span_row(i + 1, i + 1, "s", common::now_ns())])
}

async fn eventually(store: &Store, cf: &str, expected: usize) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if store.count(cf).unwrap() >= expected {
            return;
        }
        assert!(Instant::now() < deadline, "{cf} never reached {expected}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn watermark_triggers_a_flush_before_the_interval() {
    let dir = tmp_dir("watermark");
    let store = Arc::new(Store::open(&dir.path().join("data")).unwrap());
    // interval is far too long to be what flushed
    let (pipeline, rx, queued) = Pipeline::<SpanBatch>::channel(10);
    let handle = tokio::spawn(pipeline::writer(
        rx,
        store.clone(),
        Queue {
            capacity: 10,
            watermark: 5,
            flush_ms: 30_000,
        },
        queued,
    ));
    for i in 0..5u8 {
        pipeline.send(span_batch(i)).unwrap();
    }
    eventually(&store, "spans_by_trace", 5).await;
    handle.abort();
}

#[tokio::test]
async fn interval_triggers_a_flush_without_reaching_the_watermark() {
    let dir = tmp_dir("interval");
    let store = Arc::new(Store::open(&dir.path().join("data")).unwrap());
    let (pipeline, rx, queued) = Pipeline::<SpanBatch>::channel(10);
    let handle = tokio::spawn(pipeline::writer(
        rx,
        store.clone(),
        Queue {
            capacity: 10,
            watermark: 100,
            flush_ms: 150,
        },
        queued,
    ));
    pipeline.send(span_batch(0)).unwrap();
    eventually(&store, "spans_by_trace", 1).await;
    handle.abort();
}

#[tokio::test]
async fn closing_the_channel_drains_everything() {
    let dir = tmp_dir("drain");
    let store = Arc::new(Store::open(&dir.path().join("data")).unwrap());
    let (pipeline, rx, queued) = Pipeline::<SpanBatch>::channel(10);
    let handle = tokio::spawn(pipeline::writer(
        rx,
        store.clone(),
        Queue {
            capacity: 10,
            watermark: 100,
            flush_ms: 60_000,
        },
        queued,
    ));
    for i in 0..3u8 {
        pipeline.send(span_batch(i)).unwrap();
    }
    drop(pipeline);
    handle.await.unwrap();
    assert_eq!(
        store.count("spans_by_trace").unwrap(),
        3,
        "drained on close"
    );
}

#[tokio::test]
async fn full_queue_rejects_with_full_error() {
    let dir = tmp_dir("full");
    let store = Arc::new(Store::open(&dir.path().join("data")).unwrap());
    // capacity 2, no writer running: nothing drains the queue
    let (pipeline, _rx, _queued) = Pipeline::<SpanBatch>::channel(2);
    pipeline.send(span_batch(0)).expect("first send fits");
    pipeline.send(span_batch(1)).expect("second send fits");
    let attempt = pipeline.send(span_batch(2));
    assert!(
        matches!(attempt, Err(mpsc::error::TrySendError::Full(_))),
        "third send must be rejected as Full"
    );
    // and nothing was written
    assert_eq!(store.count("spans_by_trace").unwrap(), 0);
}
