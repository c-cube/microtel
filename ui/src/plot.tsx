import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
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
  onPick?: (idx: number, label: string) => void;
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
        const w = Math.max(1, Math.min(48, slot * 0.8));

        ctx.save();
        ctx.beginPath();
        ctx.rect(u.bbox.left, u.bbox.top, u.bbox.width, u.bbox.height);
        ctx.clip();
        const hovered = u.cursor.idx;
        if (hovered != null && hovered < n) {
          ctx.fillStyle = "rgba(128,140,160,0.18)";
          ctx.fillRect(px(hovered) - slot / 2, u.bbox.top, slot, u.bbox.height);
        }
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
  const [hidden, setHidden] = createSignal<Record<string, boolean>>({});
  // uPlot clears the cursor when the pointer leaves the canvas, which also
  // happens when it moves onto the tooltip; hiding is deferred so the pointer
  // has time to reach the tooltip and keep its links usable
  let overTip = false;
  let hideTimer = 0;
  const showTip = () => {
    clearTimeout(hideTimer);
    tip.style.display = "block";
  };
  const hideTipSoon = () => {
    clearTimeout(hideTimer);
    hideTimer = window.setTimeout(() => {
      if (!overTip) tip.style.display = "none";
    }, 320);
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
              const idx = u.cursor.idx;
              if (idx == null || !u.data[0]?.[idx]) {
                hideTipSoon();
                return;
              }
              let html = `<div>${fmtWhen(u.data[0][idx])}</div>`;
              for (let s = 1; s < u.series.length; s++) {
                if (!u.series[s].show) continue;
                const label = String(u.series[s].label ?? "");
                const v = (u.data[s]?.[idx] as number | null) ?? null;
                const term = props.bars ? " spans" : "";
                const dot = color(paletteIdx.get(label) ?? 0);
                html += `<div class="row"><span class="sw" style="background:${dot}"></span>${label} <span class="dim">·</span> ${
                  v == null ? "–" : fmt(v) + term
                }</div>`;
                const ex = props.tipExemplar?.(idx, label);
                if (ex) {
                  const href = props.tipLink?.(idx, label);
                  html += href
                    ? `<div class="row"><a href="${href}">↳ ${ex}</a></div>`
                    : `<div class="row"><span class="dim">↳ ${ex}</span></div>`;
                }
              }
              tip.innerHTML = html;
              showTip();
              const left = u.cursor.left ?? 0;
              const flip = left > u.bbox.width - 300;
              const gap = 24;
              const tipBox = tip.parentElement;
              const maxLeft = Math.max(0, (tipBox?.clientWidth ?? box.clientWidth) - tip.offsetWidth);
              const preferredLeft = flip ? left - tip.offsetWidth - gap : left + gap;
              tip.style.left = `${Math.min(maxLeft, Math.max(0, preferredLeft))}px`;
              tip.style.top = `${Math.min(Math.max(0, (tipBox?.clientHeight ?? box.clientHeight) - tip.offsetHeight), Math.max(0, (u.cursor.top ?? 0) - 18))}px`;
            },
          },
        },
      ],
    };

    up = new uPlot(opts, [props.t, ...series.map((s) => s.values)], host);

    // the tooltip carries the exemplar link, so it stays alive while the
    // pointer is over the tooltip itself, even after leaving the canvas
    tip.addEventListener("mouseenter", () => {
      overTip = true;
      clearTimeout(hideTimer);
    });
    tip.addEventListener("mouseleave", () => {
      overTip = false;
      hideTipSoon();
    });
    box.addEventListener("mouseleave", () => {
      if (!overTip) hideTipSoon();
    });

    if (props.onPick && props.bars) {
      up.over.addEventListener("click", () => {
        const idx = up?.cursor.idx;
        if (idx == null || !up) return;
        const ypx = up.cursor.top ?? -1;
        let below = 0;
        for (let s = 1; s < up.series.length; s++) {
          const ser = up.series[s];
          const v = (up.data[s]?.[idx] as number | null) ?? 0;
          if (!ser.show || v <= 0) {
            if (ser.show) below += v;
            continue;
          }
          const yTop = up.valToPos(below + v, "y", true);
          const yBot = up.valToPos(below, "y", true);
          if (ypx >= yTop && ypx <= yBot) {
            props.onPick?.(idx, String(ser.label ?? ""));
            return;
          }
          below += v;
        }
      });
    }
  };

  onMount(() => {
    build();
    const ro = new ResizeObserver(() => {
      const w = host.clientWidth;
      if (up && w > 0) up.setSize({ width: w, height: props.height ?? 380 });
    });
    ro.observe(host);
    onCleanup(() => {
      ro.disconnect();
      up?.destroy();
    });
  });

  createEffect(() => {
    props.t;
    props.series;
    hidden();
    resolved();
    build();
  });

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
