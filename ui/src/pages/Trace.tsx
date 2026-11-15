import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { A, useParams, useSearchParams } from "@solidjs/router";
import { PALETTE } from "../plot";
import { fmtDur, fmtWhen, trace, type TraceSpan } from "../api";

const KIND = ["internal", "internal", "server", "client", "producer", "consumer"];

interface Row {
  span: TraceSpan;
  depth: number;
}

// 0, step, 2*step... on round time units so the ruler reads naturally
function niceTicks(spanNs: number) {
  const target = 4;
  const raw = spanNs / target;
  const mag = 10 ** Math.floor(Math.log10(raw));
  const norm = raw / mag;
  const step = (norm <= 1 ? 1 : norm <= 2 ? 2 : norm <= 5 ? 5 : 10) * mag;
  const ticks: number[] = [];
  for (let v = 0; v <= spanNs * 1.0001; v += step) ticks.push(v / spanNs);
  return ticks;
}

function ruler(ticks: number[], spanNs: number) {
  return (
    <div class="wf-ruler">
      <For each={ticks}>
        {(frac) => (
          <span class="wf-tick" style={{ left: `${Math.min(frac * 100, 99.6)}%` }}>
            {frac === 0 ? "0" : fmtDur(Math.round(spanNs * frac))}
          </span>
        )}
      </For>
    </div>
  );
}

function RowView(props: {
  row: Row;
  t0: number;
  spanNs: number;
  color: string;
  open: boolean;
  onToggle: () => void;
}) {
  const s = () => props.row.span;
  const left = () => ((s().start_ns - props.t0) / props.spanNs) * 100;
  const width = () => Math.max((s().duration_ns / props.spanNs) * 100, 0.35);
  const attrs = () => Object.entries(s().attrs);

  return (
    <>
      <div
        class={`wf-row ${s().error ? "err" : ""}`}
        style={{ cursor: "pointer" }}
        onClick={props.onToggle}
        title={s().error && s().status_message ? s().status_message : s().span_id}
      >
        <div class="wf-name" style={{ "padding-left": `${props.row.depth * 14}px` }}>
          <span class="dot" style={{ background: props.color }} />
          <span class="wf-kind">{KIND[s().kind] ?? "internal"}</span>
          <span class="txt">{s().name}</span>
          <span class="svc">{s().service}</span>
        </div>
        <div class="wf-track">
          <div
            class={`wf-bar ${s().error ? "err" : ""}`}
            style={{ left: `${left()}%`, width: `${width()}%`, background: props.color }}
          />
        </div>
        <div class="wf-dur">{fmtDur(s().duration_ns)}</div>
      </div>
      <Show when={props.open}>
        <div class="wf-attrs">
          <span class="k">span_id</span>
          <span>{s().span_id}</span>
          <Show when={s().parent_span_id}>
            <span class="k">parent</span>
            <span>{s().parent_span_id}</span>
          </Show>
          <Show when={s().status_message}>
            <span class="k">status</span>
            <span>{s().status_message}</span>
          </Show>
          <For each={attrs()}>
            {([k, v]) => (
              <>
                <span class="k">{k}</span>
                <span>{v}</span>
              </>
            )}
          </For>
          <Show when={attrs().length === 0}>
            <span class="k">attrs</span>
            <span class="muted">none</span>
          </Show>
        </div>
      </Show>
    </>
  );
}

export default function Trace() {
  const params = useParams<{ traceId: string }>();
  const [search] = useSearchParams();

  const [data] = createResource(
    () => [params.traceId, search.at] as const,
    ([id, at]) => trace(id, at != null ? Number(at) : undefined),
  );

  // flat spans -> waterfall rows: children under parents, orphans adopted by
  // the root, start-sorted; a visited set keeps crafted cycles finite
  const rows = createMemo(() => {
    const spans = data();
    if (!spans) return [];
    const byId = new Map(spans.map((s) => [s.span_id, s]));
    const kids = new Map<string, TraceSpan[]>();
    const roots: TraceSpan[] = [];
    for (const s of spans) {
      const p = s.parent_span_id;
      if (p && byId.has(p)) {
        const list = kids.get(p);
        if (list) list.push(s);
        else kids.set(p, [s]);
      } else {
        roots.push(s);
      }
    }
    const byStart = (a: TraceSpan, b: TraceSpan) => a.start_ns - b.start_ns;
    const out: Row[] = [];
    const seen = new Set<string>();
    const walk = (s: TraceSpan, depth: number) => {
      if (seen.has(s.span_id)) return;
      seen.add(s.span_id);
      out.push({ span: s, depth });
      for (const c of (kids.get(s.span_id) ?? []).sort(byStart)) walk(c, depth + 1);
    };
    roots.sort(byStart).forEach((s) => walk(s, 0));
    for (const s of [...spans].sort(byStart)) if (!seen.has(s.span_id)) out.push({ span: s, depth: 0 });
    return out;
  });

  const t0 = () => (data() ?? []).reduce((m, s) => Math.min(m, s.start_ns), Infinity);
  const t1 = () => Math.max((data() ?? []).reduce((m, s) => Math.max(m, s.end_ns), 0), t0() + 1);
  const spanNs = () => Math.max(t1() - t0(), 1);

  const services = createMemo(() => [...new Set((data() ?? []).map((s) => s.service))].sort());
  const colorOf = (svc: string) => PALETTE[Math.max(0, services().indexOf(svc)) % PALETTE.length];

  const errors = () => (data() ?? []).filter((s) => s.error).length;

  const [open, setOpen] = createSignal<Set<string>>(new Set());
  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  const [copied, setCopied] = createSignal(false);
  const copyId = () => {
    void navigator.clipboard.writeText(params.traceId);
    setCopied(true);
    setTimeout(() => setCopied(false), 1600);
  };

  const ticks = createMemo(() => niceTicks(spanNs()));

  return (
    <div>
      <div class="toolbar">
        <A href="/spans" class="link">
          ← spans
        </A>
      </div>

      <section class="card">
        <div class="card-head">
          <span class="card-title">Trace</span>
          <button class="trace-id" onClick={copyId} title="copy trace id">
            {params.traceId}
          </button>
          <Show when={copied()}>
            <span class="card-sub">copied</span>
          </Show>
          <div class="stats">
            <div class="stat">
              <b>{data()?.length ?? 0}</b>
              <span>spans</span>
            </div>
            <div class="stat">
              <b>{data() ? fmtDur(t1() - t0()) : "–"}</b>
              <span>wall</span>
            </div>
            <div class="stat">
              <b>{errors()}</b>
              <span>errors</span>
            </div>
          </div>
        </div>

        <Show
          when={!data.loading}
          fallback={<progress class="progress is-small" max="100" style={{ margin: "16px" }} />}
        >
          <Show
            when={data() !== null && data() !== undefined}
            fallback={
              <div class="empty">
                trace not found — it may have aged out, or the <span class="mono">at</span> hint is off
              </div>
            }
          >
            <Show when={data()}>
              {(spans) => (
                <>
                  <div class="chips">
                    <span class="card-sub">{fmtWhen(t0() / 1_000_000)}</span>
                    <For each={services()}>
                      {(svc) => (
                        <span class="chip" style={{ color: colorOf(svc), cursor: "default" }}>
                          <span class="dot" />
                          {svc}
                        </span>
                      )}
                    </For>
                  </div>

                  <div class="wf">
                    <div class="wf-head">
                      <span>span</span>
                      {ruler(ticks(), spanNs())}
                      <span style={{ "text-align": "right" }}>duration</span>
                    </div>
                    <For each={rows()}>
                      {(row) => (
                        <RowView
                          row={row}
                          t0={t0()}
                          spanNs={spanNs()}
                          color={colorOf(row.span.service)}
                          open={open().has(row.span.span_id)}
                          onToggle={() => toggle(row.span.span_id)}
                        />
                      )}
                    </For>
                    <Show when={spans().length === 0}>
                      <div class="empty">empty trace</div>
                    </Show>
                  </div>
                </>
              )}
            </Show>
          </Show>
        </Show>
      </section>
    </div>
  );
}
