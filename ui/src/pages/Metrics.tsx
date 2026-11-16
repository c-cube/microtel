import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import Plot from "../plot";
import { PRESETS, REFRESH, DEFAULT_RANGE_MS, TO_PAD_MS, svcsList, stringList } from "../params";
import { fmtCount, points, series as fetchSeries } from "../api";

const fmtBytes = (v: number) => {
  if (v >= 1_073_741_824) return `${(v / 1_073_741_824).toFixed(1)} GB`;
  if (v >= 1_048_576) return `${(v / 1_048_576).toFixed(1)} MB`;
  if (v >= 1024) return `${(v / 1024).toFixed(1)} kB`;
  return String(Math.round(v));
};

const formatter = (name: string) => {
  if (/memory|bytes|heap/i.test(name)) return fmtBytes;
  if (/duration|latency/i.test(name)) return (v: number) => `${v < 10 ? v.toFixed(2) : Math.round(v)} ms`;
  if (/percent|ratio/i.test(name)) return (v: number) => `${v.toFixed(1)}%`;
  return (v: number) => fmtCount(v);
};

function MetricCard(props: { name: string; from: number; to: number; svcs: string[]; tick: number }) {
  const query = createMemo(() => {
    const q = new URLSearchParams({ name: props.name, from: String(props.from), to: String(props.to) });
    if (props.svcs.length) q.set("svcs", JSON.stringify(props.svcs));
    return q.toString();
  });
  const [pts, { refetch }] = createResource(query, (q) => points(q));
  createEffect(on(() => props.tick, () => refetch(), { defer: true }));

  const lines = () =>
    (pts()?.lines ?? []).map((l) => ({
      label: l.label.replace(/^service\.name=/, "").replace(/ .*/, ""),
      values: l.y,
    }));
  const empty = () => !pts.loading && lines().every((l) => l.values.every((v) => v == null));

  return (
    <section class="card mcard">
      <div class="card-head">
        <span class="card-title">{props.name}</span>
        <span class="card-sub">
          {lines().length} series
          <Show when={pts()?.truncated}> · capped</Show>
        </span>
      </div>
      <Show when={!empty()} fallback={<Show when={!pts.loading}><div class="empty">no points in range</div></Show>}>
        <Plot t={pts()?.t ?? []} series={lines()} fmt={formatter(props.name)} height={170} sync="metrics" />
      </Show>
    </section>
  );
}

export default function Metrics() {
  const [params, setParams] = useSearchParams();

  const fromMs = () => Number(params.from ?? Date.now() - DEFAULT_RANGE_MS);
  const toMs = () => Number(params.to ?? Date.now() + TO_PAD_MS);
  const svcs = () => svcsList(params.svcs);
  const selectedNames = () => stringList(params.metrics ?? params.names);
  const rf = () => Number(params.rf ?? 60_000);

  const [allSeries] = createResource(fetchSeries);
  const allNames = () => [...new Set((allSeries() ?? []).map((s) => s.name))].sort();
  const names = () => selectedNames().length
    ? allNames().filter((name) => selectedNames().includes(name))
    : allNames();
  const services = () => [...new Set((allSeries() ?? []).map((s) => s.attrs["service.name"]).filter((s): s is string => Boolean(s)))].sort();

  createEffect(() => {
    if (!params.from || !params.to) setParams({ from: fromMs(), to: toMs(), rf: rf() }, { replace: true });
  });

  return <MetricsBody names={names()} allNames={allNames()} services={services()} selectedNames={selectedNames()} fromMs={fromMs()} toMs={toMs()} svcs={svcs()} rf={rf()} setParams={setParams} />;
}

function MetricsBody(props: {
  names: string[];
  allNames: string[];
  services: string[];
  selectedNames: string[];
  fromMs: number;
  toMs: number;
  svcs: string[];
  rf: number;
  setParams: ReturnType<typeof useSearchParams>[1];
}) {
  const [tick, setTick] = createSignal(0);
  createEffect(
    on(
      () => props.rf,
      (m) => {
        if (!m) return;
        const id = setInterval(() => setTick((n) => n + 1), m);
        onCleanup(() => clearInterval(id));
      },
    ),
  );
  const span = () => props.toMs - props.fromMs;
  const activePreset = () => PRESETS.find(([, ms]) => Math.abs(span() - TO_PAD_MS - ms) < 2000 || Math.abs(span() - ms) < 2000)?.[0];
  const setRange = (ms: number) =>
    props.setParams({ from: Date.now() - ms, to: Date.now() + TO_PAD_MS }, { replace: true });
  const addName = (name: string) => {
    if (name) props.setParams({ metrics: JSON.stringify([...new Set([...props.selectedNames, name])]), names: undefined }, { replace: true });
  };
  const removeName = (name: string) => {
    const next = props.selectedNames.filter((item) => item !== name);
    props.setParams({ metrics: next.length ? JSON.stringify(next) : undefined, names: undefined }, { replace: true });
  };
  const addService = (service: string) => {
    if (service) props.setParams({ svcs: JSON.stringify([...new Set([...props.svcs, service])]) }, { replace: true });
  };
  const removeService = (service: string) => {
    const next = props.svcs.filter((item) => item !== service);
    props.setParams({ svcs: next.length ? JSON.stringify(next) : undefined }, { replace: true });
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
        <select
          class="ctl"
          value={props.rf}
          onChange={(e) => props.setParams({ rf: Number(e.currentTarget.value) }, { replace: true })}
        >
          <For each={REFRESH}>{(r) => <option value={r[1]}>refresh {r[0]}</option>}</For>
        </select>
        <span class="lbl">{props.names.length} metrics</span>
        <select class="ctl" value="" aria-label="Add metric filter" onChange={(e) => { addName(e.currentTarget.value); e.currentTarget.value = ""; }}>
          <option value="" disabled>filter metric…</option>
          <For each={props.allNames.filter((n) => !props.selectedNames.includes(n))}>{(n) => <option value={n}>{n}</option>}</For>
        </select>
        <select class="ctl" value="" aria-label="Add service filter" onChange={(e) => { addService(e.currentTarget.value); e.currentTarget.value = ""; }}>
          <option value="" disabled>filter service…</option>
          <For each={props.services.filter((s) => !props.svcs.includes(s))}>{(s) => <option value={s}>{s}</option>}</For>
        </select>
      </div>
      <div class="chips" aria-label="Active metric filters">
        <For each={props.selectedNames}>{(name) => <span class="chip"><span class="chip-kind">metric</span>{name}<button class="chip-x" aria-label={`Remove ${name} metric filter`} onClick={() => removeName(name)}>×</button></span>}</For>
        <For each={props.svcs}>{(svc) => <span class="chip"><span class="chip-kind">service</span>{svc}<button class="chip-x" aria-label={`Remove ${svc} service filter`} onClick={() => removeService(svc)}>×</button></span>}</For>
      </div>
      <Show when={props.names.length > 0} fallback={<div class="empty">{props.allNames.length ? "no metrics match these filters" : "no metrics ingested yet"}</div>}>
        <div class="mgrid">
          <For each={props.names}>
            {(n) => <MetricCard name={n} from={props.fromMs} to={props.toMs} svcs={props.svcs} tick={tick()} />}
          </For>
        </div>
      </Show>
    </div>
  );
}
