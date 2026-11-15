/* @refresh reload */
import { render } from "solid-js/web";
import { Navigate, Route, Router } from "@solidjs/router";
import Layout from "./App";
import Spans from "./pages/Spans";
import Metrics from "./pages/Metrics";
import Trace from "./pages/Trace";

render(
  () => (
    <Router>
      <Route path="/" component={Layout}>
        <Route path="/" component={() => <Navigate href="/spans" />} />
        <Route path="/spans" component={Spans} />
        <Route path="/metrics" component={Metrics} />
        <Route path="/trace/:traceId" component={Trace} />
      </Route>
    </Router>
  ),
  document.getElementById("root")!,
);
