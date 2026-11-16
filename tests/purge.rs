mod common;

use std::path::PathBuf;
use std::time::Duration;

use microtel::keys;
use prost::Message;

/// ttl_days: 1 → cutoff = yesterday; spans older than yesterday are purged daily (here: every 1s).
#[test]
fn purge_expired_by_ttl() {
    let repo_tmp = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".tmp");
    std::fs::create_dir_all(&repo_tmp).unwrap();
    let dir = tempfile::Builder::new()
        .prefix("microtel-purge-")
        .tempdir_in(&repo_tmp)
        .unwrap();
    let data_dir = dir.path().join("data");
    let cfg_path = dir.path().join("microtel.yaml");
    std::fs::write(
        &cfg_path,
        "listen: 127.0.0.1:0\nttl_days: 1\npurge_interval_secs: 1\n",
    )
    .unwrap();
    let (mut child, addr, _ui) = common::spawn_server(&data_dir, Some(&cfg_path));
    let base = format!("http://{addr}");

    let post_trace = |req: Vec<u8>| -> u16 {
        match ureq::post(&format!("{base}/v1/traces"))
            .set("Content-Type", "application/x-protobuf")
            .send_bytes(&req)
        {
            Ok(r) => r.status(),
            Err(ureq::Error::Status(code, _)) => code,
            Err(e) => panic!("transport error: {e}"),
        }
    };

    let now = common::now_ns();
    let three_days_ago = now - 3 * 86_400_000_000_000;
    let req = common::trace_request(vec![common::span(
        [9u8; 16],
        [1u8; 8],
        "stale",
        three_days_ago,
    )]);
    assert_eq!(post_trace(req.encode_to_vec()), 200);
    let req = common::trace_request(vec![common::span([8u8; 16], [1u8; 8], "fresh", now)]);
    assert_eq!(post_trace(req.encode_to_vec()), 200);

    // wait past a purge tick so the stale span (flushed within 1s) is removed
    std::thread::sleep(Duration::from_millis(3000));
    let status = common::terminate(&mut child);
    assert!(
        status.success(),
        "graceful shutdown exited nonzero: {status}"
    );

    let db = common::open_read_only(&data_dir);
    assert_eq!(
        common::cf_count(&db, "spans_by_trace"),
        1,
        "stale span purged"
    );
    assert_eq!(
        common::cf_count(&db, "spans_by_time"),
        1,
        "stale index purged"
    );

    let cf = db.cf_handle("spans_by_trace").unwrap();
    let (key, value) = db
        .iterator_cf(cf, rocksdb::IteratorMode::Start)
        .next()
        .unwrap()
        .unwrap();
    let envelope = common::SpanEnvelope::decode(&value[..]).unwrap();
    assert_eq!(envelope.span.unwrap().name, "fresh");
    assert_eq!(
        u32::from_be_bytes(key[..4].try_into().unwrap()),
        keys::ymd(now),
        "surviving row carries today's date"
    );
}
