use microtel::keys;

#[allow(clippy::inconsistent_digit_grouping)] // YYYYMMDD literals read better grouped
#[test]
fn ymd_matches_known_dates() {
    let day = |d: u64| d * keys::NS_PER_DAY;
    assert_eq!(keys::ymd(day(0)), 1970_01_01);
    // 2000-03-01 = day 11017 (2000 is a leap year)
    assert_eq!(keys::ymd(day(11_017)), 2000_0301);
    // 2026-11-13 = day 20770
    assert_eq!(keys::ymd(day(20_770)), 2026_1113);
    // 2100-03-01 = day 47541 (2100 is NOT a leap year)
    assert_eq!(keys::ymd(day(47_541)), 2100_0301);
    // end-of-day still lands on the same date
    assert_eq!(keys::ymd(day(20_770) + keys::NS_PER_DAY - 1), 2026_1113);
}

#[test]
fn varint_roundtrip() {
    let cases = [
        0u64,
        1,
        127,
        128,
        300,
        16_383,
        16_384,
        u32::MAX as u64,
        u64::MAX,
    ];
    for v in cases {
        let mut buf = Vec::new();
        keys::varint(v, &mut buf);
        let (decoded, n) = keys::read_varint(&buf).expect("decodes");
        assert_eq!(decoded, v);
        assert_eq!(n, buf.len(), "varint must be canonical");
    }
}

#[test]
fn read_varint_rejects_truncated() {
    assert_eq!(keys::read_varint(&[0x80]), None, "continuation without end");
    assert_eq!(keys::read_varint(&[]), None, "empty");
    let mut overflowing = vec![0x80u8; 10];
    overflowing.push(0x01);
    assert_eq!(keys::read_varint(&overflowing), None, "11 bytes > u64");
}

#[test]
fn span_time_value_roundtrip() {
    let ids: Vec<&[u8]> = vec![&[1u8; 8], &[2u8; 8], &[3u8; 8]];
    let v = keys::span_time_value(ids.clone());
    let parsed = keys::parse_span_ids(&v).expect("parses");
    assert_eq!(parsed, ids);
    assert_eq!(v.len(), 1 + 3 * keys::SPAN_ID_LEN);
    assert_eq!(keys::span_time_value([]), vec![0x00], "empty list");
}

#[test]
fn parse_span_ids_rejects_bad_lengths() {
    assert_eq!(keys::parse_span_ids(&[]), None);
    // claims 2 ids but only carries 1
    assert_eq!(
        keys::parse_span_ids(&[0x02, 0u8, 0, 0, 0, 0, 0, 0, 0]),
        None
    );
}

#[test]
fn series_prefix_layout() {
    let hash = [7u8; 16];
    let k = keys::series_prefix("", &hash);
    assert_eq!(
        k,
        [0x00]
            .iter()
            .chain(hash.iter())
            .copied()
            .collect::<Vec<u8>>()
    );
    assert_eq!(keys::series_prefix_len(&k), Some(17));

    let long_name = "x".repeat(200);
    let k = keys::series_prefix(&long_name, &hash);
    assert_eq!(k.len(), 1 + 200 + 16 + 1, "200 > 127 needs a 2-byte varint");
    assert_eq!(keys::series_prefix_len(&k), Some(k.len()));

    let name = "http.server.duration";
    let k = keys::series_prefix(name, &hash);
    assert_eq!(keys::series_prefix_len(&k), Some(k.len()));
    let (decoded, n) = keys::read_varint(&k).expect("decodes");
    let decoded = decoded as usize;
    assert_eq!(decoded, name.len());
    assert_eq!(&k[n..n + decoded], name.as_bytes());
}

#[test]
fn point_key_layout_and_ordering() {
    let hash = [3u8; 16];
    let t1 = 1_700_000_000_000_000_000u64;
    let t2 = t1 + 1_000;
    let a = keys::point_key("m", &hash, keys::ymd(t1), t1);
    let b = keys::point_key("m", &hash, keys::ymd(t2), t2);
    assert_eq!(a.len(), 1 + 1 + 16 + 4 + 8);
    assert!(a < b, "later points sort after earlier ones");
    assert!(b.starts_with(&a[..a.len() - 12]), "same series prefix");
    // keys for different names must not share prefixes
    let c = keys::point_key("n", &hash, keys::ymd(t1), t1);
    assert_ne!(a, c);
    assert!(a < c || c < a);
}

#[test]
fn span_keys_layout() {
    let trace = [1u8; 16];
    let k = keys::span_trace_key(2026_1113, &trace, &[2u8; 8]);
    assert_eq!(k.len(), 4 + 16 + 8);
    assert_eq!(&k[..4], &2026_1113_u32.to_be_bytes());
    assert_eq!(&k[4..20], &trace);
    assert_eq!(&k[20..], &[2u8; 8]);

    let t = 1_700_000_000_000_000_000u64;
    let tk = keys::span_time_key(keys::ymd(t), t, &trace);
    assert_eq!(tk.len(), 4 + 8 + 16);
    assert_eq!(&tk[4..12], &t.to_be_bytes());
}
