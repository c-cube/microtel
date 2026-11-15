export const PRESETS: [string, number][] = [
  ["15m", 900_000],
  ["1h", 3_600_000],
  ["3h", 10_800_000],
  ["6h", 21_600_000],
  ["1d", 86_400_000],
  ["3d", 259_200_000],
  ["7d", 604_800_000],
];

export const REFRESH: [string, number][] = [
  ["off", 0],
  ["10s", 10_000],
  ["30s", 30_000],
  ["1m", 60_000],
  ["5m", 300_000],
  ["30m", 1_800_000],
];

export const DEFAULT_RANGE_MS = 3_600_000;

// the newest bucket stays visible with a small right pad
export const TO_PAD_MS = 60_000;

export function svcsList(raw: unknown): string[] {
  return typeof raw === "string" && raw.length > 0
    ? raw.split(",").filter(Boolean)
    : [];
}
