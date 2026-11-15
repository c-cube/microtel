import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import Plot, { palette } from "../plot";
import { PRESETS, REFRESH, DEFAULT_RANGE_MS, TO_PAD_MS, svcsList } from "../params";
import { barchart, fmtCount, type GroupBy } from "../api";

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
      setParams({ from: fromMs(), to: toMs(), rf: rf() }, { replace: true });
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

  createEffect(
    on(rf, (m) => {
      if (!m) return;
      const id = setInterval(refetch, m);
      onCleanup(() => clearInterval(id));
    }),
  );

  const setRange = (ms: number) =>
    setParams({ from: Date.now() - ms, to: Date.now() + TO_PAD_MS }, { replace: true });

  const activePreset = () => PRESETS.find(([, ms]) => Math.abs(toMs() - fromMs() - ms) < 1000)?.[0];

  const findExemplar = (idx: number, label: string) => {
    const c = chart();
    if (!c) return undefined;
    const t = c.buckets[0]?.[idx];
    return c.exemplars.find((e) => e.t === t && e.label === label);
  };

  const exemplarText = (idx: number, label: string) => {
    const ex = findExemplar(idx, label);
    return ex ? `trace ${ex.trace_id.slice(0, 12)}…` : undefined;
  };

  const exemplarHref = (idx: number, label: string) => {
    const ex = findExemplar(idx, label);
    return ex ? `/trace/${ex.trace_id}?at=${ex.t}` : undefined;
  };

  const [copied, setCopied] = createSignal<{ text: string; href?: string }>({ text: "" });
  let toastTimer: ReturnType<typeof setTimeout> | undefined;
  createEffect(() => {
    if (copied().text) {
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => setCopied({ text: "" }), 4000);
    }
  });

  const pickExemplar = (idx: number, label: string) => {
    const ex = findExemplar(idx, label);
    if (ex) void navigator.clipboard.writeText(ex.trace_id);
    setCopied(
      ex
        ? {
            text: `${label} · ${ex.trace_id.slice(0, 12)}… copied`,
            href: `/trace/${ex.trace_id}?at=${ex.t}`,
          }
        : { text: "no exemplar in that bucket" },
    );
  };

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

  const totals = createMemo(() => {
    const c = chart();
    if (!c) return { sum: 0, peak: 0, services: 0 };
    let sum = 0;
    let peak = 0;
    for (let i = 0; i < (c.buckets[0]?.length ?? 0); i++) {
      let col = 0;
      for (let s = 1; s < c.buckets.length; s++) col += c.buckets[s][i] ?? 0;
      sum += col;
      if (col > peak) peak = col;
    }
    return {
      sum,
      peak,
      services: c.labels.filter((l) => l !== "other").length,
    };
  });

  const perLabel = (label: string) => {
    const c = chart();
    const i = c?.labels.indexOf(label) ?? -1;
    if (!c || i < 0) return 0;
    return (c.buckets[1 + i] ?? []).reduce<number>((a, v) => a + (v ?? 0), 0);
  };

  const bucketLabel = () => {
    const ms = chart()?.bucket_ms ?? 0;
    if (ms >= 3_600_000) return `${ms / 3_600_000}h`;
    if (ms >= 60_000) return `${ms / 60_000}m`;
    return `${ms / 1000}s`;
  };

  return (
    <div>
      <div class="toolbar">
        <div class="seg">
          <For each={PRESETS}>
            {([name, ms]) => (
              <button class={activePreset() === name ? "on" : ""} onClick={() => setRange(ms)}>
                {name}
              </button>
            )}
          </For>
        </div>
        <span class="lbl">group</span>
        <select
          class="ctl"
          value={group()}
          onChange={(e) => setParams({ group: e.currentTarget.value }, { replace: true })}
        >
          <For each={GROUPS}>{(g) => <option value={g}>{g}</option>}</For>
        </select>
        <select
          class="ctl"
          value={rf()}
          onChange={(e) => setParams({ rf: Number(e.currentTarget.value) }, { replace: true })}
        >
          <For each={REFRESH}>{(r) => <option value={r[1]}>refresh {r[0]}</option>}</For>
        </select>
        <Show when={chart()?.truncated}>
          <span class="badge warn">capped at 50k spans</span>
        </Show>
      </div>

      <section class="card">
        <div class="card-head">
          <span class="card-title">Spans over time</span>
          <span class="card-sub">
            {bucketLabel()} buckets
            {svcs().length ? ` · ${svcs().length} service filter${svcs().length > 1 ? "s" : ""}` : ""}
          </span>
          <div class="stats">
            <div class="stat">
              <b>{fmtCount(totals().sum)}</b>
              <span>spans</span>
            </div>
            <div class="stat">
              <b>{fmtCount(totals().peak)}</b>
              <span>peak / bucket</span>
            </div>
            <div class="stat">
              <b>{totals().services}</b>
              <span>services</span>
            </div>
          </div>
        </div>

        <Show when={chart() && chart()!.labels.length > 0 && group() !== "none"}>
          <div class="legend">
            <For each={chart()!.labels.filter((l) => l !== "other")}>
              {(label) => (
                <button
                  class={`lrow ${svcs().includes(label) ? "on" : ""}`}
                  onClick={() => toggleSvc(label)}
                  title="filter by service"
                >
                  <span class="sw" style={{ background: colorOf(label) }} />
                  <span class="nm">{label}</span>
                  <span class="v">{fmtCount(perLabel(label))}</span>
                  <span class="p">{totals().sum ? `${((perLabel(label) / totals().sum) * 100).toFixed(0)}%` : ""}</span>
                </button>
              )}
            </For>
            <Show when={svcs().length > 0}>
              <button class="lrow" onClick={() => setParams({ svcs: undefined }, { replace: true })}>
                <span />
                <span class="nm">clear filter</span>
                <span />
                <span />
              </button>
            </Show>
          </div>
        </Show>

        <Show
          when={chart() && totals().sum > 0}
          fallback={
            <Show when={!chart.loading}>
              <div class="empty">no spans in this range — try a wider window</div>
            </Show>
          }
        >
          <Plot
            t={chart()?.buckets[0] ?? []}
            series={plotSeries()}
            bars
            legend={false}
            height={420}
            tipExemplar={exemplarText}
            tipLink={exemplarHref}
            onPick={pickExemplar}
          />
        </Show>
      </section>

      <Show when={copied().text}>
        <div class="toast on">
          {copied().text}
          <Show when={copied().href}>
            <a class="link" style={{ "margin-left": "10px", color: "#a5b4fc" }} href={copied().href}>
              open trace →
            </a>
          </Show>
        </div>
      </Show>
    </div>
  );

  function colorOf(label: string) {
    const c = chart();
    const i = c ? c.labels.indexOf(label) : 0;
    return palette()[i % palette().length];
  }
}
