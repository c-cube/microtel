import { createEffect, on, onCleanup } from "solid-js";
import uPlot from "uplot";
import { fmtWhen } from "./api";
import { resolved } from "./theme";
import type { Heatmap } from "./api";

// Same tip styles as plot.tsx (.tip), so heatmaps match the line/bar cards.
export default function HeatmapPlot(props: {
  heat: Heatmap;
  fmt: (v: number) => string;
  height?: number;
  sync?: string;
}) {
  let host!: HTMLDivElement;
  let tip!: HTMLDivElement;
  let up: uPlot | undefined;

  const bounds = () => props.heat.bounds;
  const rows = () => bounds().length + 1;
  const maxCount = () => {
    let max = 0;
    for (const row of props.heat.counts)
      for (const c of row) if (c > max) max = c;
    return max;
  };
  // edge rows get half a neighbour interval of breathing room
  const yRange = (): [number, number] => {
    const b = bounds();
    if (!b.length) return [0, 1];
    const firstGap = (b[1] ?? b[0] + 1) - b[0];
    const lastGap = b[b.length - 1] - (b[b.length - 2] ?? b[b.length - 1] - 1);
    return [b[0] - firstGap / 2, b[b.length - 1] + lastGap / 2];
  };
  const rowInterval = (r: number): [number, number] => {
    const b = bounds();
    return [r === 0 ? b[0] : b[r - 1], r === b.length ? b[b.length - 1] : b[r]];
  };
  const rowAt = (value: number) => bounds().filter((x) => x < value).length;

  const build = () => {
    if (up) {
      up.destroy();
      up = undefined;
    }
    const heat = props.heat;
    if (!heat.t.length || !bounds().length) return;
    const max = maxCount();
    const [y0, y1] = yRange();
    const spanMs = heat.t.length > 1 ? heat.t[heat.t.length - 1] - heat.t[0] : 3_600_000;
    const css = getComputedStyle(document.documentElement);
    const cssVar = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
    const brand = cssVar("--brand", "#0a56c8");
    const muted = cssVar("--muted", "#4a5563");
    const line = cssVar("--line", "#b8c0cc");
    const lineSoft = cssVar("--line-soft", "#e3e7ec");
    const fmt = props.fmt;
    const b = bounds();
    // too many gridlines overlap; thin them
    const step = Math.max(1, Math.ceil(b.length / 7));
    const grid = b.filter((_, i) => i % step === 0);

    const heatBars = (): uPlot.Plugin => ({
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
          for (let col = 0; col < n; col++) {
            const x = px(col) - w / 2;
            if (x > u.bbox.left + u.bbox.width) break;
            for (let row = 0; row < rows(); row++) {
              const c = heat.counts[row]?.[col] ?? 0;
              if (!c) continue;
              const [lo, hi] = rowInterval(row);
              const yTop = u.valToPos(hi, "y", true);
              const yBot = u.valToPos(lo, "y", true);
              const h = Math.max(1, yBot - yTop);
              ctx.globalAlpha = 0.12 + 0.88 * Math.sqrt(c / max);
              ctx.fillStyle = brand;
              ctx.fillRect(x, yTop, w, h);
            }
          }
          ctx.restore();
        },
      },
    });

    const clock = (ms: number) => {
      const d = new Date(ms);
      const p = (n: number) => String(n).padStart(2, "0");
      return spanMs > 36 * 3_600_000
        ? `${d.getMonth() + 1}/${d.getDate()} ${p(d.getHours())}:${p(d.getMinutes())}`
        : `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
    };

    const opts: uPlot.Options = {
      width: host.clientWidth || 800,
      height: props.height ?? 170,
      padding: [6, 10, 0, 0],
      scales: { x: { time: true }, y: { range: () => [y0, y1] } },
      ms: 1,
      series: [{}],
      legend: { show: false },
      cursor: {
        points: { show: false },
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
          values: (_u, splits) => splits.map(clock),
        },
        {
          stroke: muted,
          font: "11px IBM Plex Mono, monospace",
          size: 76,
          grid: { stroke: lineSoft, width: 1 },
          ticks: { show: false },
          split: () => grid,
          values: (_u, splits) => splits.map((v) => fmt(v)),
        },
      ],
      plugins: [
        heatBars(),
        {
          hooks: {
            setCursor: (u) => {
              const left = u.cursor.left;
              const top = u.cursor.top;
              if (
                left == null ||
                top == null ||
                left < 0 ||
                top < u.bbox.top ||
                top > u.bbox.top + u.bbox.height
              ) {
                if (tip.style.display !== "none") tip.style.display = "none";
                return;
              }
              const n = u.data[0].length;
              const px = (i: number) => u.valToPos(u.data[0][i], "x", true);
              const slot = n > 1 ? Math.abs(px(1) - px(0)) : u.bbox.width;
              const col = Math.min(n - 1, Math.max(0, Math.round((left - px(0)) / (slot || 1))));
              const value = u.posToVal(top, "y");
              const row = rowAt(value);
              const count = heat.counts[row]?.[col] ?? 0;
              const [lo, hi] = rowInterval(row);
              let html = `<div>${fmtWhen(u.data[0][col])}</div>`;
              html += `<div class="row">${fmt(lo)} – ${fmt(hi)} <span class="dim">·</span> ${count} in bucket</div>`;
              const ex = heat.exemplars.find((e) => e.t === heat.t[col]);
              if (ex) {
                html += `<div class="row"><a href="/trace/${ex.trace_id}?at=${heat.t[col]}">↳ trace</a></div>`;
              }
              tip.innerHTML = html;
              tip.style.display = "block";
              const tipBox = tip.parentElement;
              const flip = left > u.bbox.width - Math.min(300, tip.offsetWidth);
              const gap = 12;
              const maxLeft = Math.max(0, (tipBox?.clientWidth ?? 800) - tip.offsetWidth);
              tip.style.left = `${Math.min(maxLeft, Math.max(0, flip ? left - tip.offsetWidth - gap : left + gap))}px`;
              tip.style.top = `${Math.max(0, top - 12)}px`;
            },
          },
        },
      ],
    };

    up = new uPlot(opts, [heat.t], host);
  };

  createEffect(on([() => props.heat, resolved], () => build(), { defer: true }));

  onCleanup(() => {
    up?.destroy();
    up = undefined;
  });

  return (
    <div>
      <div class="plot" style={{ height: `${props.height ?? 170}px` }}>
        <div class="plot-host" ref={host} />
        <div class="tip" ref={tip} />
      </div>
    </div>
  );
}
