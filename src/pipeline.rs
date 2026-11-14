use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;

use crate::config::Queue;
use crate::store::{DbResult, MetricBatch, SpanBatch, Store};

/// Attempts per batch before the writer gives up on it.
const MAX_ATTEMPTS: u32 = 3;

/// One signal's bounded queue. `T` is the batch type the writer applies.
pub struct Pipeline<T> {
    tx: mpsc::Sender<T>,
    queued: Arc<AtomicUsize>,
    capacity: usize,
}

impl<T> Pipeline<T> {
    /// The receiver is handed to `writer`; `queued` counts batches buffered there.
    pub fn channel(capacity: usize) -> (Pipeline<T>, mpsc::Receiver<T>, Arc<AtomicUsize>) {
        let (tx, rx) = mpsc::channel(capacity);
        let queued = Arc::new(AtomicUsize::new(0));
        (
            Pipeline {
                tx,
                queued: queued.clone(),
                capacity,
            },
            rx,
            queued,
        )
    }

    pub fn send(&self, batch: T) -> Result<(), mpsc::error::TrySendError<T>> {
        // approximate: concurrent sends can overshoot capacity slightly
        if self.queued.load(Ordering::Relaxed) >= self.capacity {
            return Err(mpsc::error::TrySendError::Full(batch));
        }
        self.queued.fetch_add(1, Ordering::Relaxed);
        match self.tx.try_send(batch) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.queued.fetch_sub(1, Ordering::Relaxed);
                Err(e)
            }
        }
    }
}

/// A batch the writer knows how to persist.
pub trait Stored: Send + 'static {
    fn store(&self, store: &Store) -> DbResult<()>;
}

impl Stored for SpanBatch {
    fn store(&self, store: &Store) -> DbResult<()> {
        store.apply_traces(self)
    }
}

impl Stored for MetricBatch {
    fn store(&self, store: &Store) -> DbResult<()> {
        store.apply_metrics(self)
    }
}

pub async fn writer<T: Stored>(
    mut rx: mpsc::Receiver<T>,
    store: Arc<Store>,
    q: Queue,
    queued: Arc<AtomicUsize>,
) {
    let flush_after = Duration::from_millis(q.flush_ms);
    let mut buf: VecDeque<T> = VecDeque::new();
    let mut oldest: Option<Instant> = None;
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            maybe = rx.recv() => match maybe {
                Some(batch) => {
                    if oldest.is_none() {
                        oldest = Some(Instant::now());
                    }
                    buf.push_back(batch);
                    if buf.len() >= q.watermark {
                        flush(&store, &mut buf, &queued).await;
                        oldest = None;
                    }
                }
                None => {
                    flush(&store, &mut buf, &queued).await;
                    return;
                }
            },
            _ = tick.tick() => {
                if let Some(t) = oldest
                    && t.elapsed() >= flush_after
                {
                    flush(&store, &mut buf, &queued).await;
                    oldest = None;
                }
            }
        }
    }
}

async fn flush<T: Stored>(store: &Store, buf: &mut VecDeque<T>, queued: &AtomicUsize) {
    let n = buf.len();
    while let Some(batch) = buf.pop_front() {
        if let Err(e) = retry(|| batch.store(store), MAX_ATTEMPTS).await {
            tracing::error!(error = %e, "flush failed after retries; batch dropped");
        }
    }
    queued.fetch_sub(n, Ordering::Relaxed);
}

async fn retry<T, F: FnMut() -> DbResult<T>>(mut f: F, max_attempts: u32) -> DbResult<T> {
    let max_attempts = max_attempts.max(1);
    let mut attempt = 1;
    loop {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < max_attempts => {
                tracing::warn!(error = %e, attempt, "write failed; retrying");
                tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}
