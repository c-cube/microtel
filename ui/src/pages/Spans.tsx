import { For, Show, createEffect, createMemo, createResource, createSignal, on, onCleanup, onMount } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import Plot, { palette } from "../plot";
import { PRESETS, REFRESH, DEFAULT_RANGE_MS, TO_PAD_MS, svcsList, stringList, jsonStringList } from "../params";
import { barchart, fmtCount, spanOptions, type GroupDimension } from "../api";

const BUILTIN_GROUPS: GroupDimension[] = ["service", "name", "status"];
const validGroup = (s: string): s is GroupDimension => {
  if (BUILTIN_GROUPS.includes(s as GroupDimension)) return true;
  const key = s.startsWith("span:") ? s.slice(5) : s.startsWith("resource:") ? s.slice(9) : "";
  return key.length > 0 && key.length <= 256;
};

export default function Spans() {
  const [params, setParams] = useSearchParams();

  const fromMs = () => Number(params.from ?? Date.now() - DEFAULT_RANGE_MS);
  const toMs = () => Number(params.to ?? Date.now() + TO_PAD_MS);
  const groups = (): GroupDimension[] => {
    const legacyParam = typeof params.group === "string" ? params.group : undefined;
    const legacy = legacyParam === "none" ? [] : [legacyParam || "service"];
    return [...new Set(jsonStringList(params.groups, legacy).filter(validGroup))].slice(0, 8);
  };
  const svcs = () => svcsList(params.svcs);
  const spanNames = () => stringList(params.snames);
  const rf = () => Number(params.rf ?? 60_000);
  const optionsQuery = createMemo(() => new URLSearchParams({ from: String(fromMs()), to: String(toMs()) }).toString());
  const [options] = createResource(optionsQuery, (q) => spanOptions(q));

  // Size the base histogram to 22px slots; the server adds about 20% more
  // buckets after snapping, making the displayed bars a little narrower.
  const MIN_SLOT_PX = 22;
  const [want, setWant] = createSignal(60);
  let cardEl: HTMLElement | undefined;
  onMount(() => {
    let timer = 0;
    const measure = () => {
      const w = (cardEl?.clientWidth ?? 0) - 90;
      if (w > 0) setWant(Math.max(8, Math.floor(w / MIN_SLOT_PX)));
    };
    measure();
    const ro = new ResizeObserver(() => {
      clearTimeout(timer);
      timer = window.setTimeout(measure, 200);
    });
    if (cardEl) ro.observe(cardEl);
    onCleanup(() => {
      clearTimeout(timer);
      ro.disconnect();
    });
  });

  onMount(() => {
    if (!params.from || !params.to) {
      setParams({ from: fromMs(), to: toMs(), rf: rf() }, { replace: true });
    }
  });

  const query = createMemo(() => {
    const q = new URLSearchParams({
      from: String(fromMs()),
      to: String(toMs()),
      groups: JSON.stringify(groups()),
      buckets: String(want()),
    });
    const s = svcs();
    if (s.length) q.set("svcs", JSON.stringify(s));
    const n = spanNames();
    if (n.length) q.set("snames", JSON.stringify(n));
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

  const toggleSvc = (label: string) => {
    const cur = svcs();
    const next = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label];
    setParams({ svcs: next.length ? JSON.stringify(next) : undefined }, { replace: true });
  };
  const addService = (service: string) => {
    if (service && !svcs().includes(service)) setParams({ svcs: JSON.stringify([...svcs(), service]) }, { replace: true });
  };
  const removeService = (service: string) => {
    const next = svcs().filter((s) => s !== service);
    setParams({ svcs: next.length ? JSON.stringify(next) : undefined }, { replace: true });
  };
  const addSpanName = (name: string) => {
    if (name && !spanNames().includes(name)) setParams({ snames: JSON.stringify([...spanNames(), name]) }, { replace: true });
  };
  const removeSpanName = (name: string) => {
    const next = spanNames().filter((s) => s !== name);
    setParams({ snames: next.length ? JSON.stringify(next) : undefined }, { replace: true });
  };
  const setGroups = (next: GroupDimension[]) =>
    setParams({ groups: JSON.stringify(next), group: undefined }, { replace: true });
  const groupLabel = (g: GroupDimension) => {
    if (g === "service") return "service";
    if (g === "name") return "span name";
    if (g === "status") return "status";
    const [scope, key] = g.split(":", 2);
    return `${scope} · ${key}`;
  };
  const tipGroupLabel = (label: string) => {
    const active = groups();
    const group = active.length === 1 ? active[0] : undefined;
    return group && label !== "other" ? `${groupLabel(group)}: ${label}` : label;
  };
  const serviceOnly = () => groups().length === 1 && groups()[0] === "service";

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
    if (!c) return { sum: 0, peak: 0, groups: 0 };
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
      groups: c.labels.filter((l) => l !== "other").length,
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
    if (ms >= 3_600_000 && ms % 3_600_000 === 0) return `${ms / 3_600_000}h`;
    if (ms >= 60_000 && ms % 60_000 === 0) return `${ms / 60_000}m`;
    if (ms >= 1_000) return `${Math.round(ms / 1_000)}s`;
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
        <select class="ctl" value="" aria-label="Add group by" onChange={(e) => {
          const value = e.currentTarget.value;
          if (value && groups().length < 8 && validGroup(value) && !groups().includes(value)) setGroups([...groups(), value]);
          e.currentTarget.value = "";
        }}>
          <option value="">add group by…</option>
          <optgroup label="Built in">
            <For each={BUILTIN_GROUPS.filter((g) => !groups().includes(g))}>{(g) => <option value={g}>{groupLabel(g)}</option>}</For>
          </optgroup>
          <optgroup label="Span attributes">
            <For each={(options()?.span_attributes ?? []).map((key) => `span:${key}` as GroupDimension).filter((g) => !groups().includes(g))}>
              {(g) => <option value={g}>{groupLabel(g)}</option>}
            </For>
          </optgroup>
          <optgroup label="Resource attributes">
            <For each={(options()?.resource_attributes ?? []).map((key) => `resource:${key}` as GroupDimension).filter((g) => !groups().includes(g))}>
              {(g) => <option value={g}>{groupLabel(g)}</option>}
            </For>
          </optgroup>
        </select>
        <select class="ctl" value="" aria-label="Filter services" onChange={(e) => {
          addService(e.currentTarget.value);
          e.currentTarget.value = "";
        }}>
          <option value="">filter service…</option>
          <For each={(options()?.services ?? []).filter((s) => !svcs().includes(s))}>{(s) => <option value={s}>{s}</option>}</For>
        </select>
        <select class="ctl" value="" aria-label="Filter span names" onChange={(e) => {
          addSpanName(e.currentTarget.value);
          e.currentTarget.value = "";
        }}>
          <option value="">filter span name…</option>
          <For each={(options()?.names ?? []).filter((s) => !spanNames().includes(s))}>{(s) => <option value={s}>{s}</option>}</For>
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

      <div class="chips" aria-label="Active span query">
        <For each={groups()}>
          {(g) => <span class="chip"><span class="chip-kind">group</span>{groupLabel(g)}<button class="chip-x" aria-label={`Remove ${groupLabel(g)} grouping`} onClick={() => setGroups(groups().filter((item) => item !== g))}>×</button></span>}
        </For>
        <For each={svcs()}>
          {(svc) => <span class="chip"><span class="chip-kind">service</span>{svc}<button class="chip-x" aria-label={`Remove ${svc} service filter`} onClick={() => removeService(svc)}>×</button></span>}
        </For>
        <For each={spanNames()}>
          {(name) => <span class="chip"><span class="chip-kind">span</span>{name}<button class="chip-x" aria-label={`Remove ${name} span filter`} onClick={() => removeSpanName(name)}>×</button></span>}
        </For>
      </div>

      <section class="card" ref={cardEl}>
        <div class="card-head">
          <span class="card-title">Spans over time</span>
          <span class="card-sub">
            {bucketLabel()} buckets
            {groups().length ? ` · grouped by ${groups().length} field${groups().length === 1 ? "" : "s"}` : ""}
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
              <b>{totals().groups}</b>
              <span>groups</span>
            </div>
          </div>
        </div>

        <Show when={chart() && chart()!.labels.length > 0 && groups().length > 0}>
          <div class="legend">
            <For each={chart()!.labels.filter((l) => l !== "other")}>
              {(label) => (
                <button
                  class={`lrow ${serviceOnly() && svcs().includes(label) ? "on" : ""}`}
                  disabled={!serviceOnly()}
                  onClick={() => serviceOnly() && toggleSvc(label)}
                  title={serviceOnly() ? "filter by service" : "service filtering is available above"}
                >
                  <span class="sw" style={{ background: colorOf(label) }} />
                  <span class="nm">{label}</span>
                  <span class="v">{fmtCount(perLabel(label))}</span>
                  <span class="p">{totals().sum ? `${((perLabel(label) / totals().sum) * 100).toFixed(0)}%` : ""}</span>
                </button>
              )}
            </For>
            <Show when={serviceOnly() && svcs().length > 0}>
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
            tipLabel={tipGroupLabel}
            tipExemplar={exemplarText}
            tipLink={exemplarHref}
          />
        </Show>
      </section>
    </div>
  );

  function colorOf(label: string) {
    const c = chart();
    const i = c ? c.labels.indexOf(label) : 0;
    return palette()[i % palette().length];
  }
}
