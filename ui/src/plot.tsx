import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import uPlot from "uplot";
import { fmtCount, fmtWhen } from "./api";
import { resolved } from "./theme";

// red is reserved for error markers (trace waterfall, badges)
const LIGHT = ["#0a56c8", "#0a8a45", "#d9480f", "#7c3aed", "#0891b2", "#c2255c", "#a67c00", "#475569", "#0d9488", "#9c36b5", "#e8590c", "#1971c2"];
const DARK = ["#5ea6ff", "#3ddc84", "#ff9a3d", "#b794ff", "#35d0e8", "#ff6fa8", "#ffd23f", "#a3b1c6", "#2dd4bf", "#e599f7", "#ffa94d", "#74c0fc"];
export const palette = () => (resolved() === "dark" ? DARK : LIGHT);
export const PALETTE = LIGHT;

export interface PlotSeries {
  label: string;
  values: (number | null)[];
}

export interface PlotProps {
  t: number[];
  series: PlotSeries[];
  bars?: boolean;
  legend?: boolean;
  height?: number;
  unit?: string;
  fmt?: (v: number) => string;
  tipExemplar?: (idx: number, label: string) => string | undefined;
  tipLink?: (idx: number, label: string) => string | undefined;
  tipLabel?: (label: string) => string;
  sync?: string;
  inlineLegend?: boolean;
}

const clock = (ms: number, spanMs: number) => {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return spanMs > 36 * 3_600_000
    ? `${d.getMonth() + 1}/${d.getDate()} ${p(d.getHours())}:${p(d.getMinutes())}`
    : spanMs > 3_600_000
      ? `${p(d.getHours())}:${p(d.getMinutes())}`
      : `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
};

const escapeHtml = (value: string) =>
  value.replace(/[&<>"']/g, (char) => {
    switch (char) {
      case "&": return "&amp;";
      case "<": return "&lt;";
      case ">": return "&gt;";
      case '"': return "&quot;";
      default: return "&#39;";
    }
  });

function segmentAt(u: uPlot, idx: number, y: number): number | undefined {
  let below = 0;
  for (let s = 1; s < u.series.length; s++) {
    const ser = u.series[s];
    if (!ser.show) continue;
    const value = (u.data[s]?.[idx] as number | null) ?? 0;
    if (value > 0) {
      const yTop = u.valToPos(below + value, "y", true);
      const yBot = u.valToPos(below, "y", true);
      if (y >= yTop && y <= yBot) return s;
    }
    below += value;
  }
  return undefined;
}

// uPlot ships no stacked bars; paint cumulative rects per visible series after
// the grid. Hidden series are skipped, so the stack reflows with the legend.
function stackedBars(): uPlot.Plugin {
  return {
    hooks: {
      draw: (u: uPlot) => {
        const ctx = u.ctx;
        const n = u.data[0].length;
        if (!n) return;
        const px = (i: number) => u.valToPos(u.data[0][i], "x", true);
        const slot = n > 1 ? Math.abs(px(1) - px(0)) : u.bbox.width;
        const w = Math.max(1, n > 1 ? slot + 1 : Math.min(48, slot));

        ctx.save();
        ctx.beginPath();
        ctx.rect(u.bbox.left, u.bbox.top, u.bbox.width, u.bbox.height);
        ctx.clip();
        for (let i = 0; i < n; i++) {
          let below = 0;
          const x = px(i) - w / 2;
          if (x < u.bbox.left - w) continue;
          for (let s = 1; s < u.series.length; s++) {
            const v = (u.data[s]?.[i] as number | null | undefined) ?? 0;
            if (!u.series[s].show) continue;
            if (v > 0) {
              const yTop = u.valToPos(below + v, "y", true);
              const yBot = u.valToPos(below, "y", true);
              const fill = u.series[s].fill as unknown;
              ctx.fillStyle =
                typeof fill === "function"
                  ? (fill as (u: uPlot, i: number) => string)(u, s)
                  : String(fill);
              ctx.fillRect(x, yTop, w, Math.max(1, yBot - yTop));
            }
            below += v;
          }
        }
        ctx.restore();
      },
    },
  };
}

export default function Plot(props: PlotProps) {
  let box!: HTMLDivElement;
  let host!: HTMLDivElement;
  let tip!: HTMLDivElement;
  let up: uPlot | undefined;
  let columnHighlight: HTMLDivElement | undefined;
  let highlightedIdx: number | null | undefined;
  let updateColumnHighlight:
    | ((plot: uPlot, idx?: number | null, force?: boolean) => void)
    | undefined;
  let tipKind: "preview" | "hover" | "pinned" | undefined;
  let tipHtml = "";
  const [hidden, setHidden] = createSignal<Record<string, boolean>>({});
  const [pinned, setPinned] = createSignal<{ label: string; time: number } | null>(null);
  // Defer hiding transient hover tips so the pointer can reach them. Bar
  // details are pinned separately and only close on Escape or an outside click.
  let overTip = false;
  let hideTimer = 0;
  const setTipHtml = (html: string) => {
    if (tipHtml === html) return;
    tipHtml = html;
    tip.innerHTML = html;
  };
  const showTip = (kind: "preview" | "hover" | "pinned") => {
    if (hideTimer) {
      clearTimeout(hideTimer);
      hideTimer = 0;
    }
    if (tipKind !== kind) {
      tip.classList.toggle("preview", kind === "preview");
      tip.classList.toggle("pinned", kind === "pinned");
      if (kind === "pinned") {
        tip.setAttribute("role", "dialog");
        tip.setAttribute("aria-label", "Span bucket details");
      } else {
        tip.setAttribute("role", "tooltip");
        tip.removeAttribute("aria-label");
      }
      tipKind = kind;
    }
    if (tip.style.display !== "block") tip.style.display = "block";
  };
  const hideTipSoon = () => {
    if (hideTimer) clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => {
      if (!pinned() && !overTip) tip.style.display = "none";
      hideTimer = 0;
    }, props.bars ? 60 : 320);
  };
  const dismissPinned = () => {
    setPinned(null);
    tip.style.display = "none";
    tip.classList.remove("pinned");
    tip.classList.remove("preview");
    tip.setAttribute("role", "tooltip");
    tip.removeAttribute("aria-label");
    tipKind = undefined;
    if (up) updateColumnHighlight?.(up, up.cursor.idx, true);
  };

  const positionTip = (u: uPlot) => {
    const left = u.cursor.left ?? 0;
    const tipBox = tip.parentElement;
    const gap = 12;
    const flip = left > u.bbox.width - Math.min(300, tip.offsetWidth);
    const maxLeft = Math.max(0, (tipBox?.clientWidth ?? box.clientWidth) - tip.offsetWidth);
    const preferredLeft = flip ? left - tip.offsetWidth - gap : left + gap;
    const top = Math.min(
      Math.max(0, (tipBox?.clientHeight ?? box.clientHeight) - tip.offsetHeight),
      Math.max(0, (u.cursor.top ?? 0) - 12),
    );
    const nextLeft = `${Math.min(maxLeft, Math.max(0, preferredLeft))}px`;
    const nextTop = `${top}px`;
    if (tip.style.left !== nextLeft) tip.style.left = nextLeft;
    if (tip.style.top !== nextTop) tip.style.top = nextTop;
  };
  const clampTip = () => {
    const tipBox = tip.parentElement;
    if (!tipBox) return;
    const left = Number.parseFloat(tip.style.left) || 0;
    const top = Number.parseFloat(tip.style.top) || 0;
    tip.style.left = `${Math.min(left, Math.max(0, tipBox.clientWidth - tip.offsetWidth))}px`;
    tip.style.top = `${Math.min(top, Math.max(0, tipBox.clientHeight - tip.offsetHeight))}px`;
  };

  const fmt = (v: number) => (props.fmt ? props.fmt(v) : fmtCount(v));
  const color = (i: number) => palette()[i % palette().length];
  const shown = (label: string) => !hidden()[label] && props.series.some((s) => s.label === label);
  const visibleSeries = createMemo(() => props.series.filter((s) => shown(s.label)));
  const totals = (label: string) =>
    props.series.find((s) => s.label === label)?.values.reduce<number>((a, v) => a + (v ?? 0), 0) ?? 0;

  const lastFmt = (vals: (number | null)[]) => {
    for (let i = vals.length - 1; i >= 0; i--) if (vals[i] != null) return fmt(vals[i] as number);
    return "–";
  };

  const toggle = (label: string) => {
    const next = { ...hidden() };
    if (next[label]) delete next[label];
    else next[label] = true;
    setHidden(next);
  };

  const yRange = (): [number, number] => {
    const series = visibleSeries();
    if (!series.length) return [0, 1];
    let min = Infinity;
    let max = -Infinity;
    for (const s of series) {
      for (const v of s.values) {
        if (v == null || !Number.isFinite(v)) continue;
        if (v < min) min = v;
        if (v > max) max = v;
      }
    }
    if (!Number.isFinite(min) || !Number.isFinite(max)) return [0, 1];
    if (props.bars) {
      let stack = 0;
      for (let i = 0; i < props.t.length; i++) {
        let sum = 0;
        for (const s of series) sum += s.values[i] ?? 0;
        if (sum > stack) stack = sum;
      }
      return [0, Math.max(stack, 1) * 1.06];
    }
    if (min === max) return [Math.min(0, min), max === 0 ? 1 : max * 1.1];
    const pad = (max - min) * 0.08;
    return [min - pad, max + pad];
  };

  const build = () => {
    columnHighlight = undefined;
    highlightedIdx = undefined;
    updateColumnHighlight = undefined;
    if (up) {
      up.destroy();
      up = undefined;
    }
    if (!props.t.length) return;
    const width = host.clientWidth || 800;
    const height = props.height ?? 380;
    const series = visibleSeries();
    const spanMs = props.t.length > 1 ? props.t[props.t.length - 1] - props.t[0] : 3_600_000;
    const paletteIdx = new Map(props.series.map((s, i) => [s.label, i]));
    updateColumnHighlight = (plot, cursorIdx = plot.cursor.idx, force = false) => {
      const highlight = columnHighlight;
      if (!highlight) return;
      const pinnedTime = pinned()?.time;
      const idx = pinnedTime == null ? cursorIdx : plot.data[0].indexOf(pinnedTime);
      const nextIdx = idx != null && idx >= 0 && idx < plot.data[0].length ? idx : null;
      if (!force && nextIdx === highlightedIdx) return;
      highlightedIdx = nextIdx;
      if (nextIdx == null) {
        if (highlight.style.display !== "none") highlight.style.display = "none";
        return;
      }
      const px = (i: number) => plot.valToPos(plot.data[0][i] as number, "x", true);
      const slot = plot.data[0].length > 1
        ? Math.abs(px(1) - px(0))
        : plot.bbox.width;
      const left = px(nextIdx) - slot / 2 - plot.bbox.left;
      const width = `${slot}px`;
      const transform = `translateX(${left}px)`;
      if (highlight.style.width !== width) highlight.style.width = width;
      if (highlight.style.transform !== transform) highlight.style.transform = transform;
      if (highlight.style.display !== "block") highlight.style.display = "block";
    };
    const [y0, y1] = yRange();
    const css = getComputedStyle(document.documentElement);
    const cssVar = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
    const muted = cssVar("--muted", "#4a5563");
    const line = cssVar("--line", "#b8c0cc");
    const lineSoft = cssVar("--line-soft", "#e3e7ec");

    const seriesOpts: uPlot.Options["series"] = [
      {},
      ...series.map((s) => {
        const i = paletteIdx.get(s.label) ?? 0;
        return props.bars
          ? {
              label: s.label,
              fill: color(i),
              stroke: color(i),
              width: 0,
              paths: () => null as unknown as uPlot.Series.Paths,
              points: { show: false },
              value: (_u: uPlot, v: number | null) => (v == null ? "–" : fmt(v)),
            }
          : {
              label: s.label,
              stroke: color(i),
              width: 1.5,
              spanGaps: true,
              fill: props.series.length === 1 ? `${color(i)}22` : undefined,
              points: { show: false },
              value: (_u: uPlot, v: number | null) => (v == null ? "–" : fmt(v)),
            };
      }),
    ];

    const opts: uPlot.Options = {
      width,
      height,
      padding: [6, 10, 0, 0],
      scales: { x: { time: true }, y: { range: () => [y0, y1] } },
      ms: 1,
      series: seriesOpts,
      legend: { show: false },
      cursor: {
        points: { show: !props.bars, size: 6 },
        focus: { prox: 24 },
        ...(props.sync ? { sync: { key: props.sync } } : {}),
        drag: { x: false, y: false },
      },
      axes: [
        {
          stroke: muted,
          font: "11px IBM Plex Mono, monospace",
          space: spanMs > 36 * 3_600_000 ? 120 : 90,
          grid: { stroke: lineSoft, width: 1 },
          ticks: { stroke: line, width: 1, size: 4 },
          values: (_u, splits) => splits.map((v) => clock(v, spanMs)),
        },
        {
          stroke: muted,
          font: "11px IBM Plex Mono, monospace",
          size: 76,
          grid: { stroke: lineSoft, width: 1 },
          ticks: { show: false },
          values: (_u, splits) => splits.map((v) => fmt(v)),
        },
      ],
      plugins: [
        ...(props.bars ? [stackedBars()] : []),
        {
          hooks: {
            setCursor: (u) => {
              if (pinned()) return;
              if (props.bars) updateColumnHighlight?.(u, u.cursor.idx);
              const idx = u.cursor.idx;
              if (idx == null || u.data[0]?.[idx] == null) {
                hideTipSoon();
                return;
              }
              const time = escapeHtml(fmtWhen(u.data[0][idx] as number));
              if (props.bars) {
                const segment = segmentAt(u, idx, u.cursor.top ?? -1);
                if (segment != null) {
                  const key = String(u.series[segment].label ?? "");
                  const label = props.tipLabel?.(key) ?? key;
                  const value = (u.data[segment]?.[idx] as number | null) ?? 0;
                  const dot = color(paletteIdx.get(key) ?? 0);
                  setTipHtml(
                    `<div>${time}</div><div class="row"><span class="sw" style="background:${dot}"></span>${escapeHtml(label)} <span class="dim">·</span> ${fmt(value)} spans</div>`,
                  );
                } else {
                  let total = 0;
                  for (let s = 1; s < u.series.length; s++) {
                    if (u.series[s].show) total += (u.data[s]?.[idx] as number | null) ?? 0;
                  }
                  setTipHtml(`<div>${time}</div><div>${fmt(total)} spans</div>`);
                }
                showTip("preview");
              } else {
                let html = `<div>${time}</div>`;
                for (let s = 1; s < u.series.length; s++) {
                  if (!u.series[s].show) continue;
                  const label = String(u.series[s].label ?? "");
                  const v = (u.data[s]?.[idx] as number | null) ?? null;
                  const dot = color(paletteIdx.get(label) ?? 0);
                  html += `<div class="row"><span class="sw" style="background:${dot}"></span>${escapeHtml(label)} <span class="dim">·</span> ${v == null ? "–" : fmt(v)}</div>`;
                  const ex = props.tipExemplar?.(idx, label);
                  if (ex) {
                    const href = props.tipLink?.(idx, label);
                    html += href
                      ? `<div class="row"><a href="${escapeHtml(href)}">↳ ${escapeHtml(ex)}</a></div>`
                      : `<div class="row"><span class="dim">↳ ${escapeHtml(ex)}</span></div>`;
                  }
                }
                setTipHtml(html);
                showTip("hover");
              }
              positionTip(u);
            },
          },
        },
      ],
    };

    up = new uPlot(opts, [props.t, ...series.map((s) => s.values)], host);

    if (props.bars) {
      columnHighlight = document.createElement("div");
      columnHighlight.className = "plot-column-highlight";
      up.under.appendChild(columnHighlight);
      updateColumnHighlight(up, up.cursor.idx, true);
      up.over.addEventListener("click", () => {
        const plot = up;
        const idx = plot?.cursor.idx;
        if (idx == null || !plot) {
          dismissPinned();
          return;
        }
        const ypx = plot.cursor.top ?? -1;
        const segment = segmentAt(plot, idx, ypx);
        if (segment == null) {
          dismissPinned();
          return;
        }
        const key = String(plot.series[segment].label ?? "");
        const label = props.tipLabel?.(key) ?? key;
        const value = (plot.data[segment]?.[idx] as number | null) ?? null;
        const dot = color(paletteIdx.get(key) ?? 0);
        const ex = props.tipExemplar?.(idx, key);
        const href = props.tipLink?.(idx, key);
        let html = `<div class="tip-time">${escapeHtml(fmtWhen(plot.data[0][idx] as number))}</div>`;
        html += `<div class="tip-group"><span class="sw" style="background:${dot}"></span>${escapeHtml(label)}</div>`;
        html += `<div class="tip-count">${value == null ? "–" : fmt(value)} spans</div>`;
        html += ex && href
          ? `<div class="tip-trace"><a href="${escapeHtml(href)}">↳ ${escapeHtml(ex)} · open trace</a></div>`
          : `<div class="tip-empty">no trace exemplar in this bucket</div>`;
        html += `<div class="tip-dismiss">Esc or click outside to close</div>`;
        setTipHtml(html);
        tip.tabIndex = -1;
        setPinned({ label: key, time: plot.data[0][idx] as number });
        showTip("pinned");
        positionTip(plot);
        updateColumnHighlight?.(plot, idx, true);
        tip.focus({ preventScroll: true });
      });
    }
  };

  onMount(() => {
    build();
    const onTipEnter = () => {
      overTip = true;
      clearTimeout(hideTimer);
    };
    const onTipLeave = () => {
      overTip = false;
      hideTipSoon();
    };
    const onBoxLeave = () => {
      if (!overTip) hideTipSoon();
    };
    const onOutsidePointerDown = (event: PointerEvent) => {
      if (!pinned()) return;
      const target = event.target;
      if (
        target instanceof Node &&
        (tip.contains(target) || (up?.over.contains(target) ?? false))
      ) return;
      dismissPinned();
    };
    const onEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape" && pinned()) dismissPinned();
    };

    tip.addEventListener("mouseenter", onTipEnter);
    tip.addEventListener("mouseleave", onTipLeave);
    box.addEventListener("mouseleave", onBoxLeave);
    document.addEventListener("pointerdown", onOutsidePointerDown, true);
    window.addEventListener("keydown", onEscape);

    const ro = new ResizeObserver(() => {
      const w = host.clientWidth;
      if (up && w > 0) {
        up.setSize({ width: w, height: props.height ?? 380 });
        if (props.bars) updateColumnHighlight?.(up, up.cursor.idx, true);
      }
      if (pinned()) clampTip();
    });
    ro.observe(host);
    onCleanup(() => {
      ro.disconnect();
      tip.removeEventListener("mouseenter", onTipEnter);
      tip.removeEventListener("mouseleave", onTipLeave);
      box.removeEventListener("mouseleave", onBoxLeave);
      document.removeEventListener("pointerdown", onOutsidePointerDown, true);
      window.removeEventListener("keydown", onEscape);
      clearTimeout(hideTimer);
      up?.destroy();
      up = undefined;
    });
  });

  createEffect(
    on([() => props.t, () => props.series, hidden, resolved], () => build(), { defer: true }),
  );

  return (
    <div ref={box}>
      <Show when={props.series.length > 0 && props.legend !== false}>
        <div class="mlegend">
          <For each={props.series}>
            {(sr, i) => (
              <button class={`mitem ${shown(sr.label) ? "" : "off"}`} onClick={() => toggle(sr.label)} title="toggle series">
                <span class="sw" style={{ background: palette()[i() % palette().length] }} />
                {sr.label}
                <span class="mv">{props.bars ? fmtCount(totals(sr.label)) : lastFmt(sr.values)}</span>
              </button>
            )}
          </For>
        </div>
      </Show>
      <div class="plot" style={{ height: `${props.height ?? 380}px` }}>
        <div class="plot-host" ref={host} />
        <div class="tip" ref={tip} />
      </div>
    </div>
  );
}
