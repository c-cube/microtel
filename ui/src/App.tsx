import { createEffect, createSignal, type ParentProps } from "solid-js";
import { A, useLocation, useNavigate } from "@solidjs/router";
import { cycleMode, mode } from "./theme";

export default function Layout(props: ParentProps) {
  const loc = useLocation();
  const nav = useNavigate();
  const [q, setQ] = createSignal("");
  const [bad, setBad] = createSignal(false);

  // the trace id lives in the path, so the box always mirrors the page
  createEffect(() => {
    const m = loc.pathname.match(/^\/trace\/([0-9a-f]{32})/i);
    setQ(m ? m[1].toLowerCase() : "");
    setBad(false);
  });

  const submit = (e: Event) => {
    e.preventDefault();
    const id = q().trim().toLowerCase().replace(/^trace\//, "");
    if (!/^[0-9a-f]{32}$/.test(id)) {
      setBad(true);
      return;
    }
    setBad(false);
    nav(`/trace/${id}`);
  };

  return (
    <>
      <header class="top">
        <A href="/spans" class="brand">
          <span class="mark" />
          microtel
        </A>
        <nav class="nav">
          <A href="/spans" activeClass="on">
            spans
          </A>
          <A href="/metrics" activeClass="on">
            metrics
          </A>
        </nav>
        <div class="top-right">
        <form class="search" onSubmit={submit}>
          <span class="lens">⌕</span>
          <input
            class={bad() ? "bad" : ""}
            placeholder="trace id"
            spellcheck={false}
            autocomplete="off"
            value={q()}
            onInput={(e) => {
              setQ(e.currentTarget.value);
              setBad(false);
            }}
          />
        </form>
        <button class="themebtn" onClick={cycleMode} title="theme: system / light / dark">
          {mode()}
        </button>
        </div>
      </header>
      <main class="page">{props.children}</main>
    </>
  );
}
