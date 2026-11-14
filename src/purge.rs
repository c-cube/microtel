//! Purge expired data from the DB.

use std::sync::Arc;
use std::time::Duration;

use time::{OffsetDateTime, SignedDuration};

use crate::store::Store;

pub async fn purge_task(store: Arc<Store>, ttl_days: u32, interval_secs: u64) {
    loop {
        let Some(cutoff) = OffsetDateTime::now_utc()
            .date()
            .checked_sub(SignedDuration::days(i64::from(ttl_days)))
        else {
            tracing::warn!(ttl_days, "ttl reaches before year 0; skipping purge");
            tokio::time::sleep(Duration::from_secs(interval_secs)).await;
            continue;
        };
        if let Err(e) = store.purge_before(cutoff) {
            tracing::warn!(error = %e, cutoff = %cutoff, "purge failed");
        }
        tokio::time::sleep(Duration::from_secs(interval_secs)).await;
    }
}
