use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use rocksdb::{
    BlockBasedOptions, ColumnFamilyDescriptor, DB, DBCompressionType, IteratorMode, MergeOperands,
    Options, WriteBatch,
};
use time::Date;

use crate::keys;

/// `spans_by_trace` key: `date | trace_id | span_id` -> `SpanEnvelope`.
const CF_SPAN_TRACE: &str = "spans_by_trace";
/// `spans_by_time` key: `date | start_ns | trace_id` -> packed span ids (merged).
const CF_SPAN_TIME: &str = "spans_by_time";
/// `resources` key: `blake3(resource proto)[..16]` -> raw `Resource` proto.
const CF_RESOURCES: &str = "resources";
/// `scopes` key: `blake3(scope proto)[..16]` -> raw `InstrumentationScope` proto.
const CF_SCOPES: &str = "scopes";
/// `series` key: `varint name_len | name | attr_hash(16)` -> `AttrSet` proto.
const CF_SERIES: &str = "series";
/// `points` key: `series prefix | date | time_ns` -> raw `NumberDataPoint` proto.
const CF_POINTS: &str = "points";

const ALL_CFS: [&str; 7] = [
    "default",
    CF_SPAN_TRACE,
    CF_SPAN_TIME,
    CF_RESOURCES,
    CF_SCOPES,
    CF_SERIES,
    CF_POINTS,
];

/// rocksdb 0.23 does not re-export a Result alias.
pub type DbResult<T> = std::result::Result<T, rocksdb::Error>;

/// Row inserted into both span column families.
pub struct SpanRow {
    /// YYYYMMDD date prefix shared by both indexes.
    pub ymd: u32,
    pub trace_id: Vec<u8>,
    pub span_id: Vec<u8>,
    pub start_ns: u64,
    pub envelope: Vec<u8>,
}

pub struct SpanBatch {
    /// Content-addressed `resources` rows: hash -> encoded `Resource`.
    pub resources: Vec<([u8; keys::HASH_LEN], Vec<u8>)>,
    /// Content-addressed `scopes` rows: hash -> encoded `InstrumentationScope`.
    pub scopes: Vec<([u8; keys::HASH_LEN], Vec<u8>)>,
    pub spans: Vec<SpanRow>,
}

/// Row inserted into `points`.
pub struct PointRow {
    pub name: String,
    pub attr_hash: [u8; keys::HASH_LEN],
    /// YYYYMMDD date prefix for purge.
    pub ymd: u32,
    pub time_ns: u64,
    /// Encoded `NumberDataPoint`.
    pub value: Vec<u8>,
}

/// Row inserted into `series`: metric identity -> encoded `AttrSet`.
pub struct SeriesRow {
    pub name: String,
    pub attr_hash: [u8; keys::HASH_LEN],
    pub value: Vec<u8>,
}

pub struct MetricBatch {
    pub series: Vec<SeriesRow>,
    pub points: Vec<PointRow>,
}

/// Union of packed span-id lists, the `spans_by_time` merge operator.
///
/// Unresolved merge operands stay on disk until a read or compaction, so any
/// reader that opens the DB without `Store::open` must register this operator.
pub fn union_span_ids(
    _key: &[u8],
    existing: Option<&[u8]>,
    operands: &MergeOperands,
) -> Option<Vec<u8>> {
    let mut ids: Vec<&[u8]> = Vec::new();
    for value in existing.into_iter().chain(operands.iter()) {
        let parsed = keys::parse_span_ids(value)?;
        for id in parsed {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Some(keys::span_time_value(ids))
}

pub struct Store {
    db: DB,
}

impl Store {
    pub fn open(dir: &Path) -> DbResult<Store> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        opts.create_missing_column_families(true);
        opts.set_compression_type(DBCompressionType::Zstd);
        opts.set_bottommost_compression_type(DBCompressionType::Zstd);
        let mut table = BlockBasedOptions::default();
        table.set_bloom_filter(10.0, false);
        opts.set_block_based_table_factory(&table);
        let cfs: Vec<_> = ALL_CFS
            .iter()
            .map(|name| {
                let mut cf_opts = opts.clone();
                if *name == CF_SPAN_TIME {
                    cf_opts.set_merge_operator_associative("span_id_union", union_span_ids);
                }
                ColumnFamilyDescriptor::new(*name, cf_opts)
            })
            .collect();
        let db = DB::open_cf_descriptors(&opts, dir, cfs)?;
        Ok(Store { db })
    }

    fn cf(&self, name: &str) -> &rocksdb::ColumnFamily {
        self.db.cf_handle(name).expect("column family exists")
    }

    pub fn count(&self, cf: &str) -> DbResult<usize> {
        let cf = self.cf(cf);
        Ok(self
            .db
            .iterator_cf(cf, IteratorMode::Start)
            .filter_map(Result::ok)
            .count())
    }

    /// `rocksdb.estimate-num-keys` per data CF, surfaced on `/stats`.
    pub fn estimate_rows(&self) -> Vec<(&'static str, u64)> {
        [
            CF_SPAN_TRACE,
            CF_SPAN_TIME,
            CF_RESOURCES,
            CF_SCOPES,
            CF_SERIES,
            CF_POINTS,
        ]
        .iter()
        .filter_map(|cf| {
            self.db
                .property_int_value_cf(self.cf(cf), "rocksdb.estimate-num-keys")
                .ok()
                .flatten()
                .map(|rows| (*cf, rows))
        })
        .collect()
    }

    pub fn get(&self, cf: &str, key: &[u8]) -> DbResult<Option<Vec<u8>>> {
        self.db.get_cf(self.cf(cf), key)
    }

    pub fn apply_traces(&self, batch: &SpanBatch) -> DbResult<()> {
        let cf_res = self.cf(CF_RESOURCES);
        let cf_scope = self.cf(CF_SCOPES);
        let cf_trace = self.cf(CF_SPAN_TRACE);
        let cf_time = self.cf(CF_SPAN_TIME);
        let mut wb = WriteBatch::default();
        // content-addressed shared rows: insert only when absent
        for (hash, bytes) in &batch.resources {
            if self.db.get_cf(cf_res, hash.as_slice())?.is_none() {
                wb.put_cf(cf_res, hash.as_slice(), bytes.as_slice());
            }
        }
        for (hash, bytes) in &batch.scopes {
            if self.db.get_cf(cf_scope, hash.as_slice())?.is_none() {
                wb.put_cf(cf_scope, hash.as_slice(), bytes.as_slice());
            }
        }
        for s in &batch.spans {
            wb.put_cf(
                cf_trace,
                keys::span_trace_key(s.ymd, &s.trace_id, &s.span_id),
                s.envelope.as_slice(),
            );
        }
        // one merge per row: the operator unions ids, so colliding spans accumulate and
        // re-exports stay idempotent without a read-modify-write
        let mut time_rows: BTreeMap<Vec<u8>, Vec<&[u8]>> = BTreeMap::new();
        for s in &batch.spans {
            let ids = time_rows
                .entry(keys::span_time_key(s.ymd, s.start_ns, &s.trace_id))
                .or_default();
            if !ids.contains(&s.span_id.as_slice()) {
                ids.push(&s.span_id);
            }
        }
        for (key, ids) in &time_rows {
            wb.merge_cf(
                cf_time,
                key.as_slice(),
                keys::span_time_value(ids.iter().copied()),
            );
        }
        self.db.write(wb)
    }

    pub fn apply_metrics(&self, batch: &MetricBatch) -> DbResult<()> {
        let cf_series = self.cf(CF_SERIES);
        let cf_points = self.cf(CF_POINTS);
        let mut wb = WriteBatch::default();
        for s in &batch.series {
            let key = keys::series_prefix(&s.name, &s.attr_hash);
            if self.db.get_cf(cf_series, key.as_slice())?.is_none() {
                wb.put_cf(cf_series, key.as_slice(), s.value.as_slice());
            }
        }
        for p in &batch.points {
            wb.put_cf(
                cf_points,
                keys::point_key(&p.name, &p.attr_hash, p.ymd, p.time_ns),
                p.value.as_slice(),
            );
        }
        self.db.write(wb)
    }

    /// Delete rows dated strictly before `cutoff`.
    pub fn purge_before(&self, cutoff: Date) -> DbResult<()> {
        let to = keys::ymd_of(cutoff).to_be_bytes();
        self.db
            .delete_range_cf(&self.cf(CF_SPAN_TRACE), &[] as &[u8], &to)?;
        self.db
            .delete_range_cf(&self.cf(CF_SPAN_TIME), &[] as &[u8], &to)?;
        // points are nested under series prefixes; range-delete each below the cutoff date
        let mut seen = HashSet::new();
        let mut prefixes: Vec<Vec<u8>> = Vec::new();
        let iter = self.db.iterator_cf(self.cf(CF_POINTS), IteratorMode::Start);
        for kv in iter {
            let (k, _) = kv?;
            if let Some(end) = keys::series_prefix_len(&k)
                && seen.insert(k[..end].to_vec())
            {
                prefixes.push(k[..end].to_vec());
            }
        }
        for pfx in prefixes {
            let mut from = pfx.clone();
            from.extend_from_slice(&[0u8; 4]);
            let mut to_p = pfx.clone();
            to_p.extend_from_slice(&to);
            self.db.delete_range_cf(&self.cf(CF_POINTS), from, to_p)?;
        }
        Ok(())
    }
}
