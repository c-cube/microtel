import type { ParentProps } from "solid-js";
import { A } from "@solidjs/router";

export default function Layout(props: ParentProps) {
  return (
    <section class="section">
      <div class="container">
        <h1 class="title is-4">microtel</h1>
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
        {props.children}
      </div>
    </section>
  );
}
