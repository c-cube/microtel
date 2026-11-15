//! Read-side scans: barchart histogram, metric series listing, metric point grid.
//!
//! Aggregation happens at query time over the stored indexes; every scan is
//! bounded by a span/point cap and reports `truncated` when the cap bites.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;

use opentelemetry_proto::tonic::common::v1::any_value;
use prost::Message as _;
use serde::Serialize;
use time::{Date, Month};

use crate::keys::{self, HASH_LEN, TRACE_ID_LEN};
use crate::proto::{AttrSet, MetricKind, NumberDataPoint, SpanEnvelope};
use crate::store::{CF_POINTS, CF_RESOURCES, CF_SERIES, CF_SPAN_TIME, CF_SPAN_TRACE, Store};

/// Live-server scan bounds; tests pass a smaller cap.
pub const SCAN_CAP: usize = 50_000;
const TOP_GROUPS: usize = 12;
const OTHER: &str = "other";

/// Millisecond steps considered for bucket sizing; smallest with <=300 buckets wins.
const STEPS_MS: &[u64] = &[
    1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 900_000,
    1_800_000, 3_600_000, 7_200_000, 10_800_000, 21_600_000, 43_200_000, 86_400_000,
];

pub fn pick_bucket_ms(range_ms: u64) -> u64 {
    STEPS_MS
        .iter()
        .find(|step| range_ms / **step <= 300)
        .copied()
        .unwrap_or(*STEPS_MS.last().expect("non-empty"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GroupBy {
    None,
    Service,
    Name,
}

impl GroupBy {
    pub fn parse(s: &str) -> Option<GroupBy> {
        match s {
            "none" => Some(GroupBy::None),
            "service" => Some(GroupBy::Service),
            "name" => Some(GroupBy::Name),
            _ => None,
        }
    }
}

#[derive(Serialize)]
pub struct Exemplar {
    pub t: u64,
    pub label: String,
    pub trace_id: String,
    pub span_id: String,
}

/// Columnar histogram: `buckets[0]` is the bucket-start ms grid, row `i` is the
/// per-bucket counts of `labels[i]`. The array is uPlot `setData`-ready.
#[derive(Serialize)]
pub struct Barchart {
    pub bucket_ms: u64,
    pub truncated: bool,
    pub labels: Vec<String>,
    pub buckets: Vec<Vec<u64>>,
    pub exemplars: Vec<Exemplar>,
}

#[derive(Serialize)]
pub struct SeriesInfo {
    pub name: String,
    pub attrs: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub struct Line {
    pub label: String,
    pub y: Vec<Option<f64>>,
}

/// All series of one metric aligned onto the shared `t` grid; `null` gaps.
#[derive(Serialize)]
pub struct Points {
    pub bucket_ms: u64,
    pub truncated: bool,
    pub t: Vec<u64>,
    pub lines: Vec<Line>,
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// YYYYMMDD back into a calendar date.
fn ymd_date(ymd: u32) -> Option<Date> {
    Date::from_calendar_date(
        (ymd / 10_000) as i32,
        Month::try_from((ymd / 100 % 100) as u8).ok()?,
        (ymd % 100) as u8,
    )
    .ok()
}

#[derive(Serialize)]
pub struct TraceSpan {
    pub trace_id: String,
    pub span_id: String,
    pub parent_span_id: Option<String>,
    pub name: String,
    pub service: String,
    pub start_ns: u64,
    pub end_ns: u64,
    pub duration_ns: u64,
    pub attrs: BTreeMap<String, String>,
}

#[derive(Debug)]
pub enum TraceError {
    BadId,
    NotFound,
}

/// A trace crossing midnight lands under several date prefixes; check the
/// hinted date and both neighbours.
pub fn trace(store: &Store, trace_id_hex: &str, at_ms: u64) -> Result<Vec<TraceSpan>, TraceError> {
    let tid = decode_hex(trace_id_hex).ok_or(TraceError::BadId)?;
    let hinted = keys::ymd(at_ms.saturating_mul(1_000_000));
    let mut dates = vec![hinted];
    if let Some(d) = ymd_date(hinted) {
        if let Some(p) = d.previous_day() {
            dates.push(keys::ymd_of(p));
        }
        if let Some(n) = d.next_day() {
            dates.push(keys::ymd_of(n));
        }
    }

    let mut svc_memo: HashMap<[u8; HASH_LEN], String> = HashMap::new();
    let mut spans = Vec::new();
    for ymd in dates {
        let seek = keys::span_trace_key(ymd, &tid, &[]);
        for (key, value) in store.scan_from(CF_SPAN_TRACE, &seek) {
            if key.len() != 28 {
                continue;
            }
            let ky = u32::from_be_bytes(key[0..4].try_into().expect("4 bytes"));
            let t = &key[4..20];
            if t != &tid[..] || ky != ymd {
                break;
            }
            let Some(env) = SpanEnvelope::decode(&value[..]).ok() else {
                continue;
            };
            let Some(span) = env.span else { continue };
            let rh: [u8; HASH_LEN] = env
                .resource_hash
                .as_slice()
                .try_into()
                .unwrap_or([0; HASH_LEN]);
            let service = match svc_memo.get(&rh) {
                Some(s) => s.clone(),
                None => {
                    let s = service_of(store, &rh).unwrap_or_else(|| "unknown".into());
                    svc_memo.insert(rh, s.clone());
                    s
                }
            };
            spans.push(TraceSpan {
                trace_id: trace_id_hex.to_string(),
                span_id: hex(&key[20..28]),
                parent_span_id: (!span.parent_span_id.is_empty())
                    .then(|| hex(&span.parent_span_id)),
                name: span.name,
                service,
                start_ns: span.start_time_unix_nano,
                end_ns: span.end_time_unix_nano,
                duration_ns: span
                    .end_time_unix_nano
                    .saturating_sub(span.start_time_unix_nano),
                attrs: attr_map(&span.attributes),
            });
        }
    }
    if spans.is_empty() {
        return Err(TraceError::NotFound);
    }
    spans.sort_by(|a, b| {
        a.start_ns
            .cmp(&b.start_ns)
            .then_with(|| a.span_id.cmp(&b.span_id))
    });
    Ok(spans)
}

fn decode_hex(s: &str) -> Option<[u8; TRACE_ID_LEN]> {
    if s.len() != TRACE_ID_LEN * 2 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; TRACE_ID_LEN];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

fn attr_map(kvs: &[opentelemetry_proto::tonic::common::v1::KeyValue]) -> BTreeMap<String, String> {
    kvs.iter()
        .filter_map(|kv| {
            let v = kv.value.as_ref()?.value.as_ref()?;
            let s = match v {
                any_value::Value::StringValue(s) => s.clone(),
                any_value::Value::BoolValue(b) => b.to_string(),
                any_value::Value::IntValue(i) => i.to_string(),
                any_value::Value::DoubleValue(d) => d.to_string(),
                _ => return None,
            };
            Some((kv.key.clone(), s))
        })
        .collect()
}

fn service_of(store: &Store, resource_hash: &[u8]) -> Option<String> {
    let raw = store.get(CF_RESOURCES, resource_hash).ok().flatten()?;
    let resource = crate::proto::Resource::decode(raw.as_slice()).ok()?;
    resource
        .attributes
        .iter()
        .find(|kv| kv.key == "service.name")
        .and_then(|kv| match kv.value.as_ref()?.value.as_ref()? {
            any_value::Value::StringValue(s) => Some(s.clone()),
            _ => None,
        })
}

/// Bucket grid start offsets in ms.
fn grid(from_ms: u64, bucket_ms: u64, n: usize) -> Vec<u64> {
    (0..n).map(|i| from_ms + i as u64 * bucket_ms).collect()
}

pub fn barchart(
    store: &Store,
    from_ms: u64,
    to_ms: u64,
    by: GroupBy,
    svcs: &[String],
    cap: usize,
) -> Barchart {
    let from_ns = from_ms * 1_000_000;
    let to_ns = to_ms * 1_000_000;
    let bucket_ms = pick_bucket_ms(to_ms - from_ms);
    let bucket_ns = bucket_ms * 1_000_000;
    let n = usize::try_from((to_ns - from_ns).div_ceil(bucket_ns)).unwrap_or(1);
    let first_ymd = keys::ymd(from_ns);
    let last_ymd = keys::ymd(to_ns - 1);
    let selected: HashSet<&str> = svcs.iter().map(String::as_str).collect();
    let need_labels = by != GroupBy::None || !selected.is_empty();

    let mut counts: HashMap<String, Vec<u64>> = HashMap::new();
    let mut exemplars: HashMap<(usize, String), [String; 2]> = HashMap::new();
    let mut svc_memo: HashMap<[u8; HASH_LEN], String> = HashMap::new();
    let mut truncated = false;
    let mut scanned = 0usize;

    let seek = keys::span_time_key(first_ymd, from_ns, &[]);
    'outer: for (key, value) in store.scan_from(CF_SPAN_TIME, &seek) {
        if key.len() != 28 {
            continue;
        }
        let ymd = u32::from_be_bytes(key[0..4].try_into().expect("4 bytes"));
        let start_ns = u64::from_be_bytes(key[4..12].try_into().expect("8 bytes"));
        let trace_id = &key[12..28];
        if ymd > last_ymd || (ymd == last_ymd && start_ns >= to_ns) {
            break;
        }
        if start_ns < from_ns {
            continue;
        }
        let ids = match keys::parse_span_ids(&value) {
            Some(ids) => ids,
            None => continue,
        };
        for span_id in ids {
            if scanned >= cap {
                truncated = true;
                break 'outer;
            }
            scanned += 1;
            let mut label = match by {
                GroupBy::None => "spans".to_string(),
                GroupBy::Service => String::new(),
                GroupBy::Name => String::new(),
            };
            let mut svc: Option<String> = None;
            if need_labels {
                let env = store
                    .get(CF_SPAN_TRACE, &keys::span_trace_key(ymd, trace_id, span_id))
                    .ok()
                    .flatten()
                    .and_then(|raw| SpanEnvelope::decode(raw.as_slice()).ok());
                if let Some(env) = env {
                    let rh: [u8; HASH_LEN] = env
                        .resource_hash
                        .as_slice()
                        .try_into()
                        .unwrap_or([0; HASH_LEN]);
                    let name = match svc_memo.get(&rh) {
                        Some(s) => s.clone(),
                        None => {
                            let s = service_of(store, &rh).unwrap_or_else(|| "unknown".into());
                            svc_memo.insert(rh, s.clone());
                            s
                        }
                    };
                    svc = Some(name.clone());
                    if let Some(span) = env.span {
                        label = match by {
                            GroupBy::None => "spans".to_string(),
                            GroupBy::Service => name,
                            GroupBy::Name => span.name,
                        };
                    }
                }
                if let Some(s) = &svc
                    && !selected.is_empty()
                    && !selected.contains(s.as_str())
                {
                    continue;
                }
            }
            let idx = usize::try_from((start_ns - from_ns) / bucket_ns).unwrap_or(0);
            let row = counts.entry(label.clone()).or_insert_with(|| vec![0; n]);
            if idx < n {
                row[idx] += 1;
                exemplars
                    .entry((idx, label.clone()))
                    .or_insert_with(|| [hex(trace_id), hex(span_id)]);
            }
        }
    }

    // Fold beyond the top groups into `other`.
    let mut totals: Vec<(String, u64)> = counts
        .iter()
        .map(|(label, row)| (label.clone(), row.iter().sum()))
        .collect();
    totals.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let folded: HashSet<String> = totals
        .iter()
        .skip(TOP_GROUPS)
        .map(|(label, _)| label.clone())
        .collect();
    let mut labels: Vec<String> = totals
        .iter()
        .take(TOP_GROUPS)
        .map(|(label, _)| label.clone())
        .collect();
    let mut buckets: Vec<Vec<u64>> = vec![grid(from_ms, bucket_ms, n)];
    for label in &labels {
        buckets.push(counts[label].clone());
    }
    if !folded.is_empty() {
        let mut other = vec![0u64; n];
        for label in &folded {
            for (i, c) in counts[label].iter().enumerate() {
                other[i] += c;
            }
        }
        labels.push(OTHER.to_string());
        buckets.push(other);
    }

    // Relabel exemplars of folded groups onto `other`, deterministically.
    let mut relabeled: Vec<(usize, String, [String; 2])> = exemplars
        .into_iter()
        .map(|((idx, label), ids)| (idx, label, ids))
        .collect();
    relabeled.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    let mut out_exemplars: Vec<Exemplar> = Vec::new();
    let mut have_other: HashSet<usize> = HashSet::new();
    for (idx, label, ids) in relabeled {
        let label = if folded.contains(&label) {
            if !have_other.insert(idx) {
                continue;
            }
            OTHER.to_string()
        } else {
            label
        };
        out_exemplars.push(Exemplar {
            t: from_ms + idx as u64 * bucket_ms,
            label,
            trace_id: ids[0].clone(),
            span_id: ids[1].clone(),
        });
    }

    Barchart {
        bucket_ms,
        truncated,
        labels,
        buckets,
        exemplars: out_exemplars,
    }
}

pub fn series(store: &Store) -> Vec<SeriesInfo> {
    let mut out: Vec<SeriesInfo> = store
        .scan_from(CF_SERIES, &[])
        .filter_map(|(key, value)| {
            let (len, n) = keys::read_varint(&key)?;
            let name = String::from_utf8(key[n..n + len as usize].to_vec()).ok()?;
            let set = AttrSet::decode(&value[..]).ok()?;
            Some(SeriesInfo {
                name,
                attrs: attr_map(&set.attributes),
            })
        })
        .collect();
    out.sort_by(|a, b| {
        a.name
            .cmp(&b.name)
            .then(format!("{:?}", a.attrs).cmp(&format!("{:?}", b.attrs)))
    });
    out
}

struct Acc {
    kind: MetricKind,
    label: String,
    skip: bool,
    cells: Vec<Cell>,
}

#[derive(Default, Clone, Copy)]
struct Cell {
    sum: f64,
    count: u64,
    last: Option<f64>,
}

pub fn points(
    store: &Store,
    name: &str,
    from_ms: u64,
    to_ms: u64,
    svcs: &[String],
    cap: usize,
) -> Points {
    let from_ns = from_ms * 1_000_000;
    let to_ns = to_ms * 1_000_000;
    let bucket_ms = pick_bucket_ms(to_ms - from_ms);
    let bucket_ns = bucket_ms * 1_000_000;
    let n = usize::try_from((to_ns - from_ns).div_ceil(bucket_ns)).unwrap_or(1);
    let first_ymd = keys::ymd(from_ns);
    let last_ymd = keys::ymd(to_ns - 1);
    let selected: HashSet<&str> = svcs.iter().map(String::as_str).collect();

    let mut accs: HashMap<[u8; HASH_LEN], Acc> = HashMap::new();
    let mut truncated = false;
    let mut scanned = 0usize;

    let seek = keys::point_key(name, &[0; HASH_LEN], first_ymd, from_ns);
    'outer: for (key, value) in store.scan_from(CF_POINTS, &seek) {
        let plen = match keys::series_prefix_len(&key) {
            Some(plen) => plen,
            None => continue,
        };
        let (len, off) = match keys::read_varint(&key) {
            Some(x) => x,
            None => continue,
        };
        let key_name = match std::str::from_utf8(&key[off..off + len as usize]) {
            Ok(s) => s.to_string(),
            Err(_) => continue,
        };
        if key_name != name {
            break;
        }
        let hash: [u8; HASH_LEN] = key[plen - HASH_LEN..plen].try_into().expect("16 bytes");
        let ymd = u32::from_be_bytes(key[plen..plen + 4].try_into().expect("4 bytes"));
        let ts = u64::from_be_bytes(key[plen + 4..plen + 12].try_into().expect("8 bytes"));
        if ymd > last_ymd || (ymd == last_ymd && ts >= to_ns) {
            break;
        }
        if ts < from_ns {
            continue;
        }
        let acc = match accs.entry(hash) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let raw = store.get(CF_SERIES, &keys::series_prefix(name, &hash));
                let set = raw
                    .ok()
                    .flatten()
                    .and_then(|v| AttrSet::decode(v.as_slice()).ok());
                let attrs = set.as_ref().map(|s| attr_map(&s.attributes));
                let svc = attrs
                    .as_ref()
                    .and_then(|a| a.get("service.name").cloned())
                    .unwrap_or_else(|| "unknown".into());
                let skip = !selected.is_empty() && !selected.contains(svc.as_str());
                e.insert(Acc {
                    kind: set
                        .as_ref()
                        .and_then(|s| MetricKind::try_from(s.kind).ok())
                        .unwrap_or(MetricKind::Unspecified),
                    label: attrs
                        .map(|a| {
                            a.iter()
                                .map(|(k, v)| format!("{k}={v}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default(),
                    skip,
                    cells: vec![Cell::default(); n],
                })
            }
        };
        if acc.skip {
            continue;
        }
        if scanned >= cap {
            truncated = true;
            break 'outer;
        }
        scanned += 1;
        let dp = match NumberDataPoint::decode(&value[..]) {
            Ok(dp) => dp,
            Err(_) => continue,
        };
        let v = match dp.value {
            Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsDouble(
                v,
            )) => v,
            Some(opentelemetry_proto::tonic::metrics::v1::number_data_point::Value::AsInt(i)) => {
                i as f64
            }
            None => continue,
        };
        let idx = usize::try_from((ts - from_ns) / bucket_ns).unwrap_or(0);
        if idx >= n {
            continue;
        }
        let cell = &mut acc.cells[idx];
        cell.sum += v;
        cell.count += 1;
        cell.last = Some(v);
    }

    let mut lines: Vec<Line> = accs
        .into_iter()
        .filter(|(_, acc)| !acc.skip && acc.cells.iter().any(|c| c.count > 0 || c.last.is_some()))
        .map(|(_, acc)| {
            let y = acc
                .cells
                .iter()
                .map(|c| match acc.kind {
                    MetricKind::Sum => c.last,
                    _ => (c.count > 0).then(|| c.sum / c.count as f64),
                })
                .collect();
            Line {
                label: acc.label,
                y,
            }
        })
        .collect();
    lines.sort_by(|a, b| a.label.cmp(&b.label));

    Points {
        bucket_ms,
        truncated,
        t: grid(from_ms, bucket_ms, n),
        lines,
    }
}
