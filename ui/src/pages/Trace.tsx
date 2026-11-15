import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { useParams, useSearchParams } from "@solidjs/router";
import { PALETTE } from "../plot";
import { fmtDur, fmtWhen, trace, type TraceSpan } from "../api";

interface Row {
  span: TraceSpan;
  depth: number;
}

function WfRow(props: {
  row: Row;
  t0: number;
  spanMs: number;
  color: string;
  open: boolean;
  onToggle: () => void;
}) {
  const s = () => props.row.span;
  const left = () => ((s().start_ns - props.t0) / props.spanMs) * 100;
  const width = () => Math.max((s().duration_ns / props.spanMs) * 100, 0.2);
  return (
    <div>
      <div
        class="wf-row"
        title="click to toggle attributes"
        style={{ cursor: "pointer", "padding-left": `${props.row.depth * 16}px` }}
        onClick={props.onToggle}
      >
        <span class="wf-name">
          {s().name} <span class="has-text-grey">{s().service}</span>
        </span>
        <div class="wf-track">
          <div
            class="wf-bar"
            style={{ left: `${left()}%`, width: `${width()}%`, background: props.color }}
          />
        </div>
        <span class="wf-dur">{fmtDur(s().duration_ns)}</span>
      </div>
      <Show when={props.open}>
        <div class="wf-attrs" style={{ "margin-left": `${props.row.depth * 16 + 8}px` }}>
          <For each={Object.entries(s().attrs)}>
            {([k, v]) => (
              <div>
                <code>{k}</code>={v}
              </div>
            )}
          </For>
          <Show when={Object.keys(s().attrs).length === 0}>
            <span class="has-text-grey">no attributes</span>
          </Show>
        </div>
      </Show>
    </div>
  );
}

export default function Trace() {
  const params = useParams<{ traceId: string }>();
  const [search] = useSearchParams();

  const at = () => (search.at != null ? Number(search.at) : undefined);
  const [data] = createResource(
    () => [params.traceId, search.at] as const,
    ([id, a]) => trace(id, a != null ? Number(a) : undefined),
  );

  // flat spans → waterfall rows: children under parents, orphans (and any
  // crafted cycles) float to the root, everything start-sorted
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
    for (const s of spans.sort(byStart)) {
      if (!seen.has(s.span_id)) out.push({ span: s, depth: 0 });
    }
    return out;
  });

  const t0 = () => (data() ?? []).reduce((m, s) => Math.min(m, s.start_ns), Infinity);
  const t1 = () =>
    Math.max((data() ?? []).reduce((m, s) => Math.max(m, s.end_ns), 0), t0() + 1);
  const spanMs = () => Math.max((t1() - t0()) / 1_000_000, 1);

  const services = () =>
    [...new Set((data() ?? []).map((s) => s.service))].sort();
  const colorOf = (svc: string) => PALETTE[services().indexOf(svc) % PALETTE.length];

  const [open, setOpen] = createSignal<Set<string>>(new Set());
  const toggle = (id: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });

  return (
    <div>
      <div class="box" style={{ "padding-top": "12px" }}>
        <Show
          when={!data.loading}
          fallback={<progress class="progress is-small is-link" max="100" />}
        >
          <Show when={data() !== null} fallback={<span class="tag is-warning">trace not found</span>}>
            <Show when={data()}>{(spans) => (
              <>
                <div class="level is-mobile" style={{ "margin-bottom": "8px" }}>
                  <div class="level-left">
                    <div class="level-item">
                      <strong>{spans().length} spans</strong>
                    </div>
                    <div class="level-item has-text-grey">{fmtDur(t1() - t0())} wall</div>
                    <div class="level-item has-text-grey">{fmtWhen(t0() / 1_000_000)}</div>
                    <div class="level-item">
                      <For each={services()}>
                        {(svc) => (
                          <span style={{ "margin-right": "8px" }}>
                            <span
                              class="swatch"
                              style={{ background: colorOf(svc) }}
                            ></span>
                            {svc}
                          </span>
                        )}
                      </For>
                    </div>
                  </div>
                </div>
                <For each={rows()}>
                  {(row) => (
                    <WfRow
                      row={row}
                      t0={t0()}
                      spanMs={spanMs()}
                      color={colorOf(row.span.service)}
                      open={open().has(row.span.span_id)}
                      onToggle={() => toggle(row.span.span_id)}
                    />
                  )}
                </For>
              </>
            )}</Show>
          </Show>
        </Show>
      </div>
    </div>
  );
}
