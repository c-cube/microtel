// Bombadil specification for the microtel UI.
//
//   bombadil browser test --headless --no-sandbox --time-limit=60s \
//     --output-path .tmp/bombadil http://127.0.0.1:4318/spans tools/bombadil/spec.ts
//
// The defaults cover uncaught exceptions, error logs, 4xx/5xx responses and
// generic clicking/typing/scrolling. The properties below pin down the layout
// and the plumbing the charts and the waterfall depend on.
import { always, extract } from "@antithesishq/bombadil";

export * from "@antithesishq/bombadil/browser/defaults";

// single-document router: only these shapes are legal, and a trace id in the
// url must be a full 32 hex chars
const path = extract((state) => state.window.location.pathname);
export const urlsAreWellFormed = always(
	() =>
		/^\/(spans|metrics)?$/.test(path.current) ||
		/^\/trace\/[0-9a-f]{32}$/.test(path.current),
);

const brand = extract(
	(state) =>
		state.document.querySelector(".top .brand")?.textContent?.trim() ?? "",
);
export const headerIsAlwaysThere = always(() => brand.current === "microtel");

// no page may render an empty main area: a card or an explicit empty-state
// message is always on screen
const main = extract((state) => {
	const el = state.document.querySelector("main.page");
	if (!el) return { content: false, text: "" };
	return {
		content: el.querySelector(".card, .empty") !== null,
		text: el.textContent?.trim() ?? "",
	};
});
export const mainAlwaysHasContent = always(
	() => main.current.content && main.current.text.length > 20,
);

// the chart canvas must fit its card: this catches sizing feedback loops
// between uPlot and the ResizeObserver
const layout = extract((state) => {
	const canvas = state.document.querySelector(".uplot canvas");
	const card = state.document.querySelector(".card");
	if (!canvas || !card) return null;
	const c = canvas.getBoundingClientRect();
	const p = card.getBoundingClientRect();
	return {
		canvasW: Math.round(c.width),
		canvasH: Math.round(c.height),
		cardW: Math.round(p.width),
		docW: state.document.documentElement.scrollWidth,
		winW: state.window.innerWidth,
	};
});
export const chartsFitTheirCard = always(
	() =>
		layout.current === null ||
		(layout.current.canvasW > 200 &&
			layout.current.canvasH > 100 &&
			layout.current.canvasW <= layout.current.cardW + 2 &&
			layout.current.docW <= layout.current.winW + 1),
);

// the waterfall only ever shows rows with a parseable, non-negative duration
const waterfall = extract((state) => {
	const rows = [...state.document.querySelectorAll(".wf-row")];
	if (rows.length === 0) return null;
	const durs = rows.map((r) => r.querySelector(".wf-dur")?.textContent ?? "");
	return {
		rows: rows.length,
		bad: durs.filter((d) => !/^\d/.test(d)).length,
	};
});
export const waterfallRowsAreSane = always(
	() => waterfall.current === null || waterfall.current.bad === 0,
);

// The metrics page is a grid now (not a metric selector): every rendered
// card should identify its metric, including while its series are loading.
const metricCards = extract((state) =>
	[...state.document.querySelectorAll(".mcard")].map((card) => ({
		title: card.querySelector(".card-title")?.textContent?.trim() ?? "",
	})),
);
export const metricCardsHaveTitles = always(() =>
	metricCards.current.every((card) => card.title.length > 0),
);

// exemplar links must point at a real trace route
const traceLink = extract((state) => {
	const a = state.document.querySelector('a[href^="/trace/"]');
	return a ? a.getAttribute("href") : null;
});
export const exemplarLinksPointAtTraces = always(
	() =>
		traceLink.current === null ||
		/^\/trace\/[0-9a-f]{32}(\?at=\d+)?$/.test(traceLink.current),
);
