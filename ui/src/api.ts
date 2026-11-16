export type GroupBy = "none" | "service" | "name";
export type GroupDimension = "service" | "name" | "status" | `span:${string}` | `resource:${string}`;

export interface SpanOptions {
  services: string[];
  names: string[];
  span_attributes: string[];
  resource_attributes: string[];
}

export interface Exemplar {
  t: number;
  label: string;
  trace_id: string;
  span_id: string;
}

export interface Barchart {
  bucket_ms: number;
  truncated: boolean;
  labels: string[];
  buckets: number[][];
  exemplars: Exemplar[];
}

export interface SeriesInfo {
  name: string;
  attrs: Record<string, string>;
}

export interface Line {
  label: string;
  y: (number | null)[];
}

export interface Points {
  bucket_ms: number;
  truncated: boolean;
  t: number[];
  lines: Line[];
}

const json = async <T,>(r: Response): Promise<T> => {
  if (!r.ok) throw new Error(`http ${r.status}`);
  return (await r.json()) as T;
};

export const barchart = (q: string) =>
  fetch(`/api/spans/barchart?${q}`).then((r) => json<Barchart>(r));

export const spanOptions = (q: string) =>
  fetch(`/api/spans/options?${q}`).then((r) => json<SpanOptions>(r));

export const series = () =>
  fetch("/api/metrics/series").then((r) => json<SeriesInfo[]>(r));

export const points = (q: string) =>
  fetch(`/api/metrics/points?${q}`).then((r) => json<Points>(r));

export interface TraceSpan {
  trace_id: string;
  span_id: string;
  parent_span_id?: string | null;
  name: string;
  service: string;
  kind: number;
  error: boolean;
  status_message: string;
  start_ns: number;
  end_ns: number;
  duration_ns: number;
  attrs: Record<string, string>;
}

// 404 resolves to null (unknown trace), other errors throw
export const trace = (id: string, at?: number) =>
  fetch(`/api/traces/${id}${at != null ? `?at=${at}` : ""}`).then((r) =>
    r.status === 404 ? null : json<TraceSpan[]>(r),
  );

export function fmtDur(ns: number): string {
  if (ns < 1_000) return `${ns}ns`;
  if (ns < 1_000_000) return `${Math.round(ns / 1_000)}\u00b5s`;
  if (ns < 1_000_000_000) return `${(ns / 1_000_000).toFixed(1)}ms`;
  const s = ns / 1_000_000_000;
  if (s < 60) return `${s.toFixed(2)}s`;
  return `${Math.floor(s / 60)}m ${Math.round(s % 60)}s`;
}

export function fmtCount(v: number): string {
  if (Math.abs(v) >= 1_000_000) return `${(v / 1_000_000).toFixed(1)}M`;
  if (Math.abs(v) >= 1_000) return `${(v / 1_000).toFixed(1)}k`;
  return String(v);
}

export function fmtWhen(ms: number): string {
  return new Date(ms).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}
