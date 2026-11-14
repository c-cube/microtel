use time::{Date, OffsetDateTime};

pub const TRACE_ID_LEN: usize = 16;
pub const SPAN_ID_LEN: usize = 8;
pub const HASH_LEN: usize = 16;
pub const NS_PER_DAY: u64 = 86_400_000_000_000;

/// UTC calendar date as YYYYMMDD. Every `u64` nanosecond value is representable.
pub fn ymd(ns: u64) -> u32 {
    let t = OffsetDateTime::from_unix_timestamp_nanos(i128::from(ns))
        .expect("u64 nanoseconds are within time's range");
    ymd_of(t.date())
}

/// YYYYMMDD for a calendar date.
pub fn ymd_of(date: Date) -> u32 {
    (date.year() as u32) * 10_000 + (date.month() as u32) * 100 + u32::from(date.day())
}

/// Append `value` as a protobuf varint.
pub fn varint(value: u64, out: &mut Vec<u8>) {
    prost::encoding::encode_varint(value, out);
}

/// Read a leading varint; returns the value and the bytes consumed.
pub fn read_varint(buf: &[u8]) -> Option<(u64, usize)> {
    let mut rest = buf;
    let value = prost::encoding::decode_varint(&mut rest).ok()?;
    Some((value, buf.len() - rest.len()))
}

/// `spans_by_trace` key: `date(4 BE) | trace_id(16) | span_id(8)`.
pub fn span_trace_key(ymd: u32, trace_id: &[u8], span_id: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(4 + trace_id.len() + span_id.len());
    k.extend_from_slice(&ymd.to_be_bytes());
    k.extend_from_slice(trace_id);
    k.extend_from_slice(span_id);
    k
}

/// `spans_by_time` key: `date(4 BE) | start_ns(8 BE) | trace_id(16)`.
pub fn span_time_key(ymd: u32, start_ns: u64, trace_id: &[u8]) -> Vec<u8> {
    let mut k = Vec::with_capacity(4 + 8 + trace_id.len());
    k.extend_from_slice(&ymd.to_be_bytes());
    k.extend_from_slice(&start_ns.to_be_bytes());
    k.extend_from_slice(trace_id);
    k
}

/// `spans_by_time` value / merge operand: `varint count | span_id(8) * count`.
pub fn span_time_value<'a>(span_ids: impl IntoIterator<Item = &'a [u8]>) -> Vec<u8> {
    let ids: Vec<&[u8]> = span_ids.into_iter().collect();
    let mut v = Vec::with_capacity(5 + ids.len() * SPAN_ID_LEN);
    varint(ids.len() as u64, &mut v);
    for id in ids {
        v.extend_from_slice(id);
    }
    v
}

/// Inverse of [`span_time_value`]; `None` if the value is truncated or oversized.
pub fn parse_span_ids(value: &[u8]) -> Option<Vec<&[u8]>> {
    let (count, n) = read_varint(value)?;
    let count = usize::try_from(count).ok()?;
    let end = n.checked_add(count.checked_mul(SPAN_ID_LEN)?)?;
    if value.len() != end {
        return None;
    }
    Some(
        (0..count)
            .map(|i| &value[n + i * SPAN_ID_LEN..n + (i + 1) * SPAN_ID_LEN])
            .collect(),
    )
}

/// Start of a `series` or `points` key: `varint name_len | name | attr_hash(16)`.
pub fn series_prefix(name: &str, attr_hash: &[u8; HASH_LEN]) -> Vec<u8> {
    let mut k = Vec::with_capacity(5 + name.len() + HASH_LEN);
    varint(name.len() as u64, &mut k);
    k.extend_from_slice(name.as_bytes());
    k.extend_from_slice(attr_hash);
    k
}

/// End offset of the series prefix in a `points` key.
///
/// Fallible because purge iterates raw stored keys: a truncated prefix or a
/// length that overflows `usize` must not panic the sweep.
pub fn series_prefix_len(key: &[u8]) -> Option<usize> {
    let (len, n) = read_varint(key)?;
    let len = usize::try_from(len).ok()?;
    let end = n.checked_add(len)?.checked_add(HASH_LEN)?;
    (end <= key.len()).then_some(end)
}

/// `points` key: `series_prefix | date(4 BE) | time_ns(8 BE)`.
pub fn point_key(name: &str, attr_hash: &[u8; HASH_LEN], ymd: u32, time_ns: u64) -> Vec<u8> {
    let mut k = series_prefix(name, attr_hash);
    k.extend_from_slice(&ymd.to_be_bytes());
    k.extend_from_slice(&time_ns.to_be_bytes());
    k
}
