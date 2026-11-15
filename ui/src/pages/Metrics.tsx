import { createEffect, createMemo, createResource, on, onCleanup } from "solid-js";
import { useSearchParams } from "@solidjs/router";
import Plot from "../plot";
import { REFRESH, DEFAULT_RANGE_MS, TO_PAD_MS, svcsList } from "../params";
import { points, series as fetchSeries } from "../api";

export default function Metrics() {
  const [params, setParams] = useSearchParams();

  const fromMs = () => Number(params.from ?? Date.now() - DEFAULT_RANGE_MS);
  const toMs = () => Number(params.to ?? Date.now() + TO_PAD_MS);
  const svcs = () => svcsList(params.svcs);
  const rf = () => Number(params.rf ?? 60_000);
  const metric = () => (typeof params.metric === "string" ? params.metric : "");

  const [allSeries] = createResource(fetchSeries);
  const names = () => [...new Set((allSeries() ?? []).map((s) => s.name))].sort();

  // pick the first name until the user chooses one
  createEffect(() => {
    const ns = names();
    if (ns.length > 0 && !ns.includes(metric())) {
      setParams({ metric: ns[0] }, { replace: true });
    }
  });

  const query = createMemo(() => {
    const q = new URLSearchParams({
      name: metric(),
      from: String(fromMs()),
      to: String(toMs()),
    });
    const s = svcs();
    if (s.length) q.set("svcs", s.join(","));
    return q.toString();
  });
  const [pts, { refetch }] = createResource(
    () => (metric() ? query() : null),
    (q) => points(q),
  );

  createEffect(
    on(rf, (m) => {
      if (!m) return;
      const id = setInterval(refetch, m);
      onCleanup(() => clearInterval(id));
    }),
  );

  const serviceValues = () =>
    [
      ...new Set(
        (allSeries() ?? [])
          .filter((s) => s.name === metric())
          .map((s) => s.attrs["service.name"])
          .filter(Boolean),
      ),
    ].sort();

  const toggleSvc = (label: string) => {
    const cur = svcs();
    const next = cur.includes(label) ? cur.filter((l) => l !== label) : [...cur, label];
    setParams({ svcs: next.length ? next.join(",") : undefined }, { replace: true });
  };

  const plotSeries = () =>
    (pts()?.lines ?? []).map((l) => ({ label: l.label, values: l.y }));

  return (
    <div>
      <div class="field is-grouped is-align-items-center">
        <div class="control">
          <div class="select is-small">
            <select
              value={metric()}
              onChange={(e) =>
                setParams({ metric: e.currentTarget.value }, { replace: true })
              }
            >
              {names().map((n) => (
                <option value={n}>{n}</option>
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
        {pts()?.truncated && <span class="tag is-warning">capped at 50k points</span>}
      </div>

      {serviceValues().length > 1 && (
        <div class="field is-grouped is-align-items-center">
          {serviceValues().map((label) => (
            <button
              class={`button is-small ${svcs().includes(label) ? "is-link" : ""}`}
              onClick={() => toggleSvc(label)}
            >
              {label}
            </button>
          ))}
        </div>
      )}

      <Plot t={pts()?.t ?? []} series={plotSeries()} />
    </div>
  );
}
