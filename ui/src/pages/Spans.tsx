import {
  createEffect,
  createMemo,
  createResource,
  createSignal,
  on,
  onCleanup,
  onMount,
} from "solid-js";
import { useSearchParams } from "@solidjs/router";
import Plot from "../plot";
import { PRESETS, REFRESH, DEFAULT_RANGE_MS, TO_PAD_MS, svcsList } from "../params";
import { barchart, type GroupBy } from "../api";

const GROUPS: GroupBy[] = ["service", "name", "none"];

export default function Spans() {
  const [params, setParams] = useSearchParams();

  const fromMs = () => Number(params.from ?? Date.now() - DEFAULT_RANGE_MS);
  const toMs = () => Number(params.to ?? Date.now() + TO_PAD_MS);
  const group = (): GroupBy =>
    GROUPS.includes(params.group as GroupBy) ? (params.group as GroupBy) : "service";
  const svcs = () => svcsList(params.svcs);
  const rf = () => Number(params.rf ?? 60_000);

  onMount(() => {
    if (!params.from || !params.to) {
      setParams(
        { from: fromMs(), to: toMs(), rf: rf() },
        { replace: true },
      );
    }
  });

  const query = createMemo(() => {
    const q = new URLSearchParams({
      from: String(fromMs()),
      to: String(toMs()),
      group_by: group(),
    });
    const s = svcs();
    if (s.length) q.set("svcs", s.join(","));
    return q.toString();
  });
  const [chart, { refetch }] = createResource(query, (q) => barchart(q));

  // refresh interval ticks refetch the current query
  createEffect(
    on(rf, (m) => {
      if (!m) return;
      const id = setInterval(refetch, m);
      onCleanup(() => clearInterval(id));
    }),
  );

  const setRange = (ms: number) =>
    setParams({ from: Date.now() - ms, to: Date.now() + TO_PAD_MS }, { replace: true });

  const activePreset = () => PRESETS.find(([, ms]) => toMs() - fromMs() === ms)?.[0];

  // exemplar lookup keyed by bucket start + group label
  const exemplarAt = (idx: number, label: string): string | undefined => {
    const c = chart();
    if (!c) return undefined;
    const t = c.buckets[0]?.[idx];
    const ex = c.exemplars.find((e) => e.t === t && e.label === label);
    return ex ? `↳ trace ${ex.trace_id.slice(0, 12)}…` : undefined;
  };

  const pickExemplar = (idx: number, label: string) => {
    const c = chart();
    if (!c) return;
    const t = c.buckets[0]?.[idx];
    const ex = c.exemplars.find((e) => e.t === t && e.label === label);
    if (ex) void navigator.clipboard.writeText(ex.trace_id);
    setCopied(`${label}: ${ex ? ex.trace_id.slice(0, 12) + "…" : "(none)"} copied`);
  };

  const [copied, setCopied] = createSignal("");
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    if (copied()) {
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => setCopied(""), 1800);
    }
  });

  const toggleSvc = (label: string) => {
    const cur = svcs();
    const next = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label];
    setParams({ svcs: next.length ? next.join(",") : undefined }, { replace: true });
  };

  const plotSeries = () => {
    const c = chart();
    if (!c) return [];
    return c.labels.map((label, i) => ({
      label,
      values: (c.buckets[1 + i] ?? []).map((v) => v as number | null),
    }));
  };

  return (
    <div>
      <div class="field is-grouped is-align-items-center">
        <div class="control">
          <div class="buttons">
            {PRESETS.map(([name, ms]) => (
              <button
                class={`button is-small ${activePreset() === name ? "is-link" : ""}`}
                onClick={() => setRange(ms)}
              >
                {name}
              </button>
            ))}
          </div>
        </div>
        <div class="control">
          <div class="select is-small">
            <select
              value={group()}
              onChange={(e) => setParams({ group: e.currentTarget.value }, { replace: true })}
            >
              {GROUPS.map((g) => (
                <option value={g}>group: {g}</option>
              ))}
            </select>
          </div>
        </div>
        <div class="control">
          <div class="select is-small">
            <select
              value={rf()}
              onChange={(e) => setParams({ rf: Number(e.currentTarget.value) }, { replace: true })}
            >
              {REFRESH.map(([name, ms]) => (
                <option value={ms}>refresh: {name}</option>
              ))}
            </select>
          </div>
        </div>
        {chart() && chart()!.truncated && (
          <span class="tag is-warning">capped at 50k spans</span>
        )}
      </div>

      {chart()?.labels.some((l) => l !== "other") && group() !== "none" && (
        <div class="field is-grouped is-align-items-center">
          {chart()!
            .labels.filter((l) => l !== "other")
            .map((label) => (
              <button
                class={`button is-small ${svcs().includes(label) ? "is-link" : ""}`}
                onClick={() => toggleSvc(label)}
              >
                {label}
              </button>
            ))}
        </div>
      )}

      <Plot
        t={chart()?.buckets[0] ?? []}
        series={plotSeries()}
        bars
        tipExemplar={exemplarAt}
        onPick={pickExemplar}
      />

      <span class="tag is-info" style={{ opacity: copied() ? 1 : 0 }}>
        {copied() || "·"}
      </span>
    </div>
  );
}
