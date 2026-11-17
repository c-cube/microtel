mod common;

use std::path::PathBuf;

use microtel::keys;
use microtel::proto::{AttrSet, hash16};
use microtel::store::Store;
use prost::Message;

fn tmp_store(tag: &str) -> (tempfile::TempDir, Store) {
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix(&format!("microtel-store-{tag}-"))
        .tempdir_in(&repo_tmp)
        .unwrap();
    let store = Store::open(&dir.path().join("data")).expect("store opens");
    (dir, store)
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

#[test]
fn spans_are_readable_through_both_indices() {
    let (_d, store) = tmp_store("readback");
    let t = now();
    store
        .apply_traces(&common::span_batch(vec![
            common::span_row(1, 1, "a", t),
            common::span_row(2, 2, "b", t + 1),
        ]))
        .unwrap();

    assert_eq!(store.count("spans_by_trace").unwrap(), 2);
    assert_eq!(store.count("spans_by_time").unwrap(), 2);
    assert_eq!(
        store.count("resources").unwrap(),
        0,
        "no resources in batch"
    );

    let key = keys::span_time_key(keys::ymd(t), t, &[1u8; 16]);
    let value = store
        .get("spans_by_time", &key)
        .unwrap()
        .expect("row exists");
    let ids = keys::parse_span_ids(&value).unwrap();
    assert_eq!(ids, vec![&[1u8; 8][..]]);

    let tkey = keys::span_trace_key(keys::ymd(t), &[1u8; 16], &[1u8; 8]);
    let envelope = store
        .get("spans_by_trace", &tkey)
        .unwrap()
        .expect("row exists");
    let decoded = common::SpanEnvelope::decode(envelope.as_slice()).unwrap();
    assert_eq!(decoded.span.as_ref().unwrap().name, "a");
    assert_eq!(decoded.resource_hash, [0xAA; 16]);
}

#[test]
fn re_exporting_a_span_is_idempotent() {
    let (_d, store) = tmp_store("idem");
    let t = now();
    let batch = common::span_batch(vec![common::span_row(1, 1, "a", t)]);
    store.apply_traces(&batch).unwrap();
    store.apply_traces(&batch).unwrap();

    assert_eq!(store.count("spans_by_trace").unwrap(), 1);
    assert_eq!(store.count("spans_by_time").unwrap(), 1);
    let key = keys::span_time_key(keys::ymd(t), t, &[1u8; 16]);
    let value = store.get("spans_by_time", &key).unwrap().unwrap();
    assert_eq!(keys::parse_span_ids(&value).unwrap().len(), 1, "no dup ids");
}

#[test]
fn colliding_spans_merge_into_one_time_row() {
    let (_d, store) = tmp_store("merge");
    let t = now();
    store
        .apply_traces(&common::span_batch(vec![
            common::span_row(1, 1, "a", t),
            common::span_row(1, 2, "b", t), // same trace, same start_ns
        ]))
        .unwrap();

    assert_eq!(store.count("spans_by_trace").unwrap(), 2);
    assert_eq!(store.count("spans_by_time").unwrap(), 1);
    let key = keys::span_time_key(keys::ymd(t), t, &[1u8; 16]);
    let value = store.get("spans_by_time", &key).unwrap().unwrap();
    let ids = keys::parse_span_ids(&value).unwrap();
    assert_eq!(ids.len(), 2);
}

#[test]
fn separate_batches_merge_into_one_time_row() {
    let (_d, store) = tmp_store("merge-batches");
    let t = now();
    // same trace and start_ns in three separate batches: the merge operator must union
    for span in [1u8, 2, 3] {
        store
            .apply_traces(&common::span_batch(vec![common::span_row(1, span, "s", t)]))
            .unwrap();
    }
    let key = keys::span_time_key(keys::ymd(t), t, &[1u8; 16]);
    let value = store.get("spans_by_time", &key).unwrap().unwrap();
    let ids = keys::parse_span_ids(&value).unwrap();
    assert_eq!(ids, vec![&[1u8; 8][..], &[2u8; 8][..], &[3u8; 8][..]]);

    // and re-exporting any of them still does not grow the row
    store
        .apply_traces(&common::span_batch(vec![common::span_row(1, 2, "s", t)]))
        .unwrap();
    let value = store.get("spans_by_time", &key).unwrap().unwrap();
    assert_eq!(keys::parse_span_ids(&value).unwrap().len(), 3);
}

#[test]
fn shared_resources_and_scopes_are_stored_once() {
    let (_d, store) = tmp_store("shared");
    let t = now();
    let resources = vec![([7u8; 16], b"resource-bytes".to_vec())];
    let scopes = vec![([8u8; 16], b"scope-bytes".to_vec())];
    let mut batch = common::span_batch(vec![common::span_row(1, 1, "a", t)]);
    batch.resources = resources;
    batch.scopes = scopes;
    store.apply_traces(&batch).unwrap();
    store.apply_traces(&batch).unwrap();

    assert_eq!(store.count("resources").unwrap(), 1);
    assert_eq!(store.count("scopes").unwrap(), 1);
    assert_eq!(
        store.get("resources", &[7u8; 16]).unwrap().unwrap(),
        b"resource-bytes"
    );
}

#[test]
fn metrics_deduplicate_series_and_points() {
    let (_d, store) = tmp_store("metrics");
    let t = now();
    let h = hash16(&AttrSet::default());
    let batch = common::metric_batch(
        vec![common::series_row("requests_total", h)],
        vec![common::point_row("requests_total", h, t)],
    );
    store.apply_metrics(&batch).unwrap();
    store.apply_metrics(&batch).unwrap();

    assert_eq!(store.count("series").unwrap(), 1);
    assert_eq!(store.count("points").unwrap(), 1);

    // same attrs on a different metric name must get its own series row
    let other = common::metric_batch(
        vec![common::series_row("errors_total", h)],
        vec![common::point_row("errors_total", h, t)],
    );
    store.apply_metrics(&other).unwrap();
    assert_eq!(store.count("series").unwrap(), 2);
    assert_eq!(store.count("points").unwrap(), 2);
}

#[test]
fn purge_drops_expired_rows_but_keeps_shared_tables() {
    let (_d, store) = tmp_store("purge");
    let now = now();
    let old = now - 3 * keys::NS_PER_DAY;
    let old_ymd = keys::ymd(old);

    store
        .apply_traces(&common::span_batch(vec![
            common::span_row(1, 1, "stale", old),
            common::span_row(2, 2, "fresh", now),
        ]))
        .unwrap();

    let h1 = [1u8; 16];
    let h2 = [2u8; 16];
    let h2_ref = &h2;
    store
        .apply_metrics(&common::metric_batch(
            vec![
                common::series_row("stale_metric", h1),
                common::series_row("live_metric", h2),
            ],
            vec![
                common::point_row("stale_metric", h1, old),
                common::point_row("live_metric", h2, now),
            ],
        ))
        .unwrap();

    store
        .purge_before(common::date_of(now - keys::NS_PER_DAY))
        .unwrap();

    assert_eq!(
        store.count("spans_by_trace").unwrap(),
        1,
        "stale span purged"
    );
    assert_eq!(
        store.count("spans_by_time").unwrap(),
        1,
        "stale index purged"
    );
    assert_eq!(
        store
            .get(
                "spans_by_trace",
                &keys::span_trace_key(old_ymd, &[1u8; 16], &[1u8; 8])
            )
            .unwrap(),
        None
    );
    assert_eq!(store.count("points").unwrap(), 1, "stale point purged");
    let point = store
        .get(
            "points",
            &keys::point_key("live_metric", h2_ref, keys::ymd(now), now),
        )
        .unwrap();
    assert!(point.is_some(), "fresh point kept");

    // series without rows left after the sweep are pruned; live series stay
    assert_eq!(store.count("series").unwrap(), 1);

    // purging again is a no-op
    store
        .purge_before(common::date_of(now - keys::NS_PER_DAY))
        .unwrap();
    assert_eq!(store.count("spans_by_trace").unwrap(), 1);
    assert_eq!(store.count("series").unwrap(), 1);
}

#[test]
fn reopening_a_store_sees_prior_writes() {
    let dir_tag = "reopen";
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix(&format!("microtel-store-{dir_tag}-"))
        .tempdir_in(&repo_tmp)
        .unwrap();
    let data_dir = dir.path().join("data");
    let t = now();
    {
        let store = Store::open(&data_dir).unwrap();
        store
            .apply_traces(&common::span_batch(vec![common::span_row(3, 3, "kept", t)]))
            .unwrap();
        drop(store);
    }
    let store = Store::open(&data_dir).unwrap();
    assert_eq!(store.count("spans_by_trace").unwrap(), 1);
}

#[test]
fn series_rows_decode_as_attr_sets() {
    let (_d, store) = tmp_store("seriesdecode");
    let h = [4u8; 16];
    store
        .apply_metrics(&common::metric_batch(
            vec![common::series_row("m", h)],
            Vec::new(),
        ))
        .unwrap();
    let key = keys::series_prefix("m", &h);
    let value = store.get("series", &key).unwrap().unwrap();
    let set = AttrSet::decode(value.as_slice()).unwrap();
    assert_eq!(set.attributes.len(), 1);
    assert_eq!(set.attributes[0].key, "k");
}
