import { createEffect, onCleanup, onMount } from "solid-js";
import uPlot from "uplot";
import { fmtCount, fmtWhen } from "./api";

export const PALETTE = [
  "#485fc7",
  "#00d1b2",
  "#f14668",
  "#ffdd57",
  "#3273dc",
  "#00947e",
  "#f68b39",
  "#b86bff",
  "#209cee",
  "#ffcc00",
  "#e64ca6",
  "#3ec486",
];

export interface PlotSeries {
  label: string;
  values: (number | null)[];
}

export interface PlotProps {
  t: number[];
  series: PlotSeries[];
  bars?: boolean;
  height?: number;
  // extra tooltip line for a (bucket, label); e.g. exemplar trace ids
  tipExemplar?: (idx: number, label: string) => string | undefined;
  // bar-segment click (spans page): copy the segment's exemplar trace id
  onPick?: (idx: number, label: string) => void;
}

// Stacked bars drawn manually: uPlot has no stacked bars; cumulative rects
// per visible series, painted after the grid. Legend toggles still work
// because hidden series are skipped and the stack reflows.
function stackedBars(): uPlot.Plugin {
  return {
    hooks: {
      draw(u: uPlot) {
        const ctx = u.ctx;
        const n = u.data[0].length;
        const px = (i: number) => u.valToPos(u.data[0][i], "x", true);
        const slot = n > 1 ? Math.abs(px(1) - px(0)) : u.bbox.width;
        const w = Math.max(1, Math.min(30, slot * 0.7));
        for (let i = 0; i < n; i++) {
          let below = 0;
          const x = px(i) - w / 2;
          if (x < u.bbox.left - w) continue;
          for (let s = 1; s < u.series.length; s++) {
            const v = (u.data[s]?.[i] as number | null | undefined) ?? 0;
            const series = u.series[s];
            if (!series.show) continue;
            if (v > 0) {
              const yTop = u.valToPos(below + v, "y", true);
              const yBottom = u.valToPos(below, "y", true);
              ctx.fillStyle = String(series.fill);
              ctx.fillRect(x, yTop, w, Math.max(1, yBottom - yTop));
            }
            below += v;
          }
        }
      },
    },
  };
}

export default function Plot(props: PlotProps) {
  let wrap!: HTMLDivElement;
  let tip!: HTMLDivElement;
  let up: uPlot | undefined;

  const paletteFill = (i: number) => PALETTE[i % PALETTE.length];

  const seriesOpts = (): uPlot.Options["series"] => [
    {},
    ...props.series.map((s, i) =>
      props.bars
        ? {
            label: s.label,
            fill: paletteFill(i),
            stroke: paletteFill(i),
            width: 0,
            paths: () => null as unknown as uPlot.Series.Paths,
            points: { show: false },
            value: (_u: uPlot, v: number | null) =>
              v == null ? "–" : fmtCount(v),
          }
        : {
            label: s.label,
            stroke: paletteFill(i),
            width: 2,
            fill: undefined,
            points: { show: false },
            value: (_u: uPlot, v: number | null) =>
              v == null ? "–" : String(Math.round(v * 100) / 100),
          },
    ),
  ];

  const stackedMax = (): number => {
    let max = 0;
    for (let i = 0; i < props.t.length; i++) {
      let sum = 0;
      for (const s of props.series) {
        const v = s.values[i];
        if (v != null) sum += v;
      }
      if (sum > max) max = sum;
    }
    return max || 1;
  };

  const build = () => {
    if (up) {
      up.destroy();
      up = undefined;
    }
    if (props.t.length === 0) return;
    const width = wrap.clientWidth || 600;
    const opts: uPlot.Options = {
      width,
      height: props.height ?? 420,
      scales: {
        x: { time: true },
        y: props.bars
          ? {
              range: (_u, _min, _max) => [0, stackedMax() * 1.02],
            }
          : { range: (_u, min, max) => [min, max] },
      },
      series: seriesOpts(),
      legend: { show: true },
      cursor: { points: { show: !props.bars } },
      plugins: [
        ...(props.bars ? [stackedBars()] : []),
        {
          hooks: {
            setCursor: (u) => {
              const idx = u.cursor.idx;
              if (idx == null) {
                tip.style.display = "none";
                return;
              }
              let html = `<b>${fmtWhen(u.data[0][idx])}</b>`;
              for (let s = 1; s < u.series.length; s++) {
                if (!u.series[s].show) continue;
                const label = String(u.series[s].label ?? "");
                const v = (u.data[s]?.[idx] as number | null) ?? null;
                html += `<br><span class="swatch" style="background:${u.series[s].fill ?? u.series[s].stroke}"></span>${label}: ${
                  v == null ? "–" : fmtCount(v)
                }`;
                const ex = props.tipExemplar?.(idx, label);
                if (ex) html += `<br><span style="opacity:.6">↳ ${ex}</span>`;
              }
              tip.innerHTML = html;
              tip.style.display = "block";
              const left = u.cursor.left ?? 0;
              const flip = left > (u.cursor.idx != null ? width - 220 : width - 220);
              tip.style.left = `${(flip ? left - 230 : left + 14)}px`;
              tip.style.top = `${Math.max(4, (u.cursor.top ?? 0) - 10)}px`;
            },
          },
        },
      ],
    };
    up = new uPlot(opts, [props.t, ...props.series.map((s) => s.values)], wrap);

    if (props.onPick && props.bars) {
      up.over.addEventListener("click", () => {
        const idx = up?.cursor.idx;
        if (idx == null || !up) return;
        const ypx = up.cursor.top ?? -1;
        let below = 0;
        for (let s = 1; s < up.series.length; s++) {
          const series = up.series[s];
          const v = (up.data[s]?.[idx] as number | null) ?? 0;
          if (!series.show || v <= 0) {
            if (series.show) below += v;
            continue;
          }
          const yTop = up.valToPos(below + v, "y", true);
          const yBottom = up.valToPos(below, "y", true);
          if (ypx >= yTop && ypx <= yBottom) {
            props.onPick?.(idx, String(series.label ?? ""));
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
      const w = wrap.clientWidth;
      if (up && w > 0) up.setSize({ width: w, height: props.height ?? 420 });
    });
    ro.observe(wrap);
    onCleanup(() => {
      ro.disconnect();
      up?.destroy();
    });
  });

  createEffect(() => {
    // recreate on every data change: labels, toggles and the tooltip all
    // rebuild cleanly; canvas init is cheap and refreshes are far apart
    props.series;
    props.t;
    build();
  });

  return (
    <div ref={wrap} style={{ position: "relative", "min-height": "420px" }}>
      <div class="chart-tip" ref={tip} />
    </div>
  );
}
