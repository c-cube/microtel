import { createEffect, createSignal, type ParentProps } from "solid-js";
import { A, useLocation, useNavigate } from "@solidjs/router";

export default function Layout(props: ParentProps) {
  const loc = useLocation();
  const nav = useNavigate();
  const [q, setQ] = createSignal("");

  // reflect the current trace id in the search box, clear elsewhere
  createEffect(() => {
    const m = loc.pathname.match(/^\/trace\/([0-9a-f]{32})/);
    setQ(m ? m[1] : "");
  });

  const go = () => {
    const id = q().trim().toLowerCase();
    if (/^[0-9a-f]{32}$/.test(id)) nav(`/trace/${id}`);
  };

  return (
    <section class="section">
      <div class="container">
        <h1 class="title is-4">microtel</h1>
        <div class="is-flex is-align-items-flex-end is-justify-content-space-between">
          <div class="tabs is-boxed">
            <ul>
              <li>
                <A href="/spans" activeClass="is-active">
                  spans
                </A>
              </li>
              <li>
                <A href="/metrics" activeClass="is-active">
                  metrics
                </A>
              </li>
            </ul>
          </div>
          <div class="field" style={{ "margin-bottom": "4px" }}>
            <div class="control">
              <input
                class="input is-small"
                style={{ width: "260px", "font-family": "monospace" }}
                placeholder="trace id"
                value={q()}
                onInput={(e) => setQ(e.currentTarget.value)}
                onKeyDown={(e) => e.key === "Enter" && go()}
              />
            </div>
          </div>
        </div>
        {props.children}
      </div>
    </section>
  );
}
