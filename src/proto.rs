pub use opentelemetry_proto::tonic::common::v1::{InstrumentationScope, KeyValue};
pub use opentelemetry_proto::tonic::resource::v1::Resource;
use opentelemetry_proto::tonic::{
    collector::{metrics::v1 as mcol, trace::v1 as tcol},
    metrics::v1 as metrics,
    trace::v1 as trace,
};
use prost::Message;

pub use mcol::ExportMetricsServiceRequest;
pub use metrics::NumberDataPoint;
pub use metrics::metric::Data;
pub use tcol::ExportTraceServiceRequest;
pub use trace::Span;

/// BLAKE3 truncated to 16 bytes over the prost encoding of `msg`.
pub fn hash16(msg: &impl Message) -> [u8; crate::keys::HASH_LEN] {
    let digest = blake3::hash(&msg.encode_to_vec());
    let mut out = [0u8; crate::keys::HASH_LEN];
    out.copy_from_slice(&digest.as_bytes()[..crate::keys::HASH_LEN]);
    out
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct SpanEnvelope {
    #[prost(bytes = "vec", tag = "1")]
    pub resource_hash: Vec<u8>,
    #[prost(bytes = "vec", tag = "2")]
    pub scope_hash: Vec<u8>,
    #[prost(message, optional, tag = "3")]
    pub span: Option<Span>,
}

#[derive(Clone, PartialEq, prost::Message)]
pub struct AttrSet {
    #[prost(message, repeated, tag = "1")]
    pub attributes: Vec<KeyValue>,
    /// Metric aggregation type, rides in the value so the attrs-only hash stays stable.
    #[prost(enumeration = "MetricKind", tag = "2")]
    pub kind: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, prost::Enumeration)]
pub enum MetricKind {
    Unspecified = 0,
    Gauge = 1,
    Sum = 2,
}
