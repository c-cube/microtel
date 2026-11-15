export type GroupBy = "none" | "service" | "name";

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

export const series = () =>
  fetch("/api/metrics/series").then((r) => json<SeriesInfo[]>(r));

export const points = (q: string) =>
  fetch(`/api/metrics/points?${q}`).then((r) => json<Points>(r));

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
