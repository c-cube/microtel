// Seeds a running microtel with a realistic demo dataset: a checkout and
// product-browse service graph with nested spans, errors, slow outliers,
// plus a backdated metric history. Node builtins only.
//
//   EMIT_ENDPOINT=http://127.0.0.1:4318 EMIT_HOURS=6 EMIT_TRACES=700 node seed.mjs

import { randomBytes } from "node:crypto";
import {
	Kind,
	metric,
	metricsRequest,
	resourceSpans,
	span,
	traceRequest,
} from "./lib/otlp.mjs";

const endpoint = process.env.EMIT_ENDPOINT ?? "http://127.0.0.1:4318";
const hours = Number(process.env.EMIT_HOURS ?? 6);
const total = Number(process.env.EMIT_TRACES ?? 700);
const nowMs = Date.now() - 10_000;
const startMs = nowMs - hours * 3_600_000;

const rand = (a, b) => a + Math.random() * (b - a);
const gauss = () => {
	let u = 0;
	let v = 0;
	while (u === 0) u = Math.random();
	while (v === 0) v = Math.random();
	return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * v);
};
// ms; log-normal around p50, with a fat tail on a few percent of calls
const latency = (p50, spread = 0.45) =>
	p50 * Math.exp(gauss() * spread) * (Math.random() < 0.02 ? rand(2, 3.5) : 1);

const S = {
	web: "web-frontend",
	gateway: "api-gateway",
	cart: "cart-service",
	pricing: "pricing-service",
	inventory: "inventory-service",
	payment: "payment-service",
	search: "search-service",
	email: "email-worker",
};

const resources = {
	[S.web]: {
		"service.name": S.web,
		"service.version": "2.14.3",
		"deployment.environment": "production",
		"host.name": "web-01",
	},
	[S.gateway]: {
		"service.name": S.gateway,
		"service.version": "4.2.0",
		"deployment.environment": "production",
		"host.name": "gw-03",
	},
	[S.cart]: {
		"service.name": S.cart,
		"service.version": "1.9.7",
		"deployment.environment": "production",
		"host.name": "cart-07",
	},
	[S.pricing]: {
		"service.name": S.pricing,
		"service.version": "3.1.2",
		"deployment.environment": "production",
		"host.name": "price-02",
	},
	[S.inventory]: {
		"service.name": S.inventory,
		"service.version": "1.4.1",
		"deployment.environment": "production",
		"host.name": "inv-05",
	},
	[S.payment]: {
		"service.name": S.payment,
		"service.version": "5.0.4",
		"deployment.environment": "production",
		"host.name": "pay-01",
	},
	[S.search]: {
		"service.name": S.search,
		"service.version": "2.2.9",
		"deployment.environment": "production",
		"host.name": "search-04",
	},
	[S.email]: {
		"service.name": S.email,
		"service.version": "0.8.2",
		"deployment.environment": "production",
		"host.name": "mail-02",
	},
};

const checkout = [
	{
		svc: S.web,
		name: "GET /checkout",
		kind: Kind.Server,
		p50: 60,
		attrs: () => ({
			"http.request.method": "GET",
			"url.path": "/checkout",
			"http.response.status_code": 200,
			"user_agent.original": "Mozilla/5.0",
		}),
	},
	{
		parent: 0,
		svc: S.gateway,
		name: "POST /api/checkout",
		kind: Kind.Server,
		p50: 48,
		attrs: () => ({
			"http.request.method": "POST",
			"url.path": "/api/checkout",
			"http.response.status_code": 200,
		}),
	},
	{
		parent: 1,
		svc: S.gateway,
		name: "cart-service/GetCart",
		kind: Kind.Client,
		p50: 9,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.service": "cart",
			"rpc.method": "GetCart",
		}),
	},
	{
		parent: 2,
		svc: S.cart,
		name: "GetCart",
		kind: Kind.Server,
		p50: 7,
		attrs: () => ({ "rpc.system": "grpc", "rpc.method": "GetCart" }),
	},
	{
		parent: 3,
		svc: S.cart,
		name: "GET cart:{id}",
		kind: Kind.Client,
		p50: 4,
		attrs: () => ({
			"db.system": "redis",
			"db.operation": "GET",
			"db.statement": "GET cart:8f21c4e0",
			"server.address": "redis-01",
		}),
	},
	{
		parent: 1,
		svc: S.gateway,
		name: "pricing-service/Quote",
		kind: Kind.Client,
		p50: 14,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.service": "pricing",
			"rpc.method": "Quote",
		}),
	},
	{
		parent: 5,
		svc: S.pricing,
		name: "Quote",
		kind: Kind.Server,
		p50: 11,
		attrs: () => ({ "rpc.system": "grpc", "rpc.method": "Quote" }),
	},
	{
		parent: 6,
		svc: S.pricing,
		name: "SELECT prices",
		kind: Kind.Client,
		p50: 5,
		attrs: () => ({
			"db.system": "postgresql",
			"db.operation": "SELECT",
			"db.statement":
				"SELECT id, amount, currency FROM prices WHERE sku = ANY($1)",
			"server.address": "pg-01",
		}),
	},
	{
		parent: 6,
		svc: S.pricing,
		name: "apply_discount_rules",
		kind: Kind.Internal,
		p50: 3,
		attrs: () => ({ "discount.rules": 12, "cart.items": 3 }),
	},
	{
		parent: 1,
		svc: S.gateway,
		name: "inventory-service/Reserve",
		kind: Kind.Client,
		p50: 11,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.service": "inventory",
			"rpc.method": "Reserve",
		}),
	},
	{
		parent: 9,
		svc: S.inventory,
		name: "Reserve",
		kind: Kind.Server,
		p50: 9,
		attrs: () => ({ "rpc.system": "grpc", "rpc.method": "Reserve" }),
	},
	{
		parent: 10,
		svc: S.inventory,
		name: "UPDATE stock",
		kind: Kind.Client,
		p50: 6,
		attrs: () => ({
			"db.system": "postgresql",
			"db.operation": "UPDATE",
			"db.statement":
				"UPDATE stock SET reserved = reserved + $1 WHERE sku = $2",
			"server.address": "pg-02",
		}),
	},
	{
		parent: 1,
		svc: S.gateway,
		name: "payment-service/Charge",
		kind: Kind.Client,
		p50: 26,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.service": "payment",
			"rpc.method": "Charge",
		}),
	},
	{
		parent: 12,
		svc: S.payment,
		name: "Charge",
		kind: Kind.Server,
		p50: 23,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.method": "Charge",
			"payment.currency": "EUR",
		}),
	},
	{
		parent: 13,
		svc: S.payment,
		name: "risk_check",
		kind: Kind.Internal,
		p50: 5,
		attrs: () => ({ "risk.score": Number(rand(0.01, 0.4).toFixed(3)) }),
	},
	{
		parent: 13,
		svc: S.payment,
		name: "POST /v1/charges",
		kind: Kind.Client,
		p50: 15,
		attrs: () => ({
			"http.request.method": "POST",
			"url.full": "https://api.stripe.com/v1/charges",
			"http.response.status_code": 200,
			"server.address": "api.stripe.com",
		}),
	},
];

const browse = [
	{
		svc: S.web,
		name: "GET /product/{id}",
		kind: Kind.Server,
		p50: 42,
		attrs: () => ({
			"http.request.method": "GET",
			"url.path": "/product/9f2a1c",
			"http.response.status_code": 200,
			"user_agent.original": "Mozilla/5.0",
		}),
	},
	{
		parent: 0,
		svc: S.gateway,
		name: "GET /api/products/{id}",
		kind: Kind.Server,
		p50: 33,
		attrs: () => ({
			"http.request.method": "GET",
			"url.path": "/api/products/9f2a1c",
			"http.response.status_code": 200,
		}),
	},
	{
		parent: 1,
		svc: S.gateway,
		name: "search-service/Search",
		kind: Kind.Client,
		p50: 26,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.service": "search",
			"rpc.method": "Search",
		}),
	},
	{
		parent: 2,
		svc: S.search,
		name: "Search",
		kind: Kind.Server,
		p50: 21,
		attrs: () => ({
			"rpc.system": "grpc",
			"rpc.method": "Search",
			"db.system": "elasticsearch",
		}),
	},
	{
		parent: 3,
		svc: S.search,
		name: "POST /products/_search",
		kind: Kind.Client,
		p50: 16,
		attrs: () => ({
			"db.system": "elasticsearch",
			"db.operation": "search",
			"server.address": "es-01",
		}),
	},
	{
		parent: 0,
		svc: S.web,
		name: "render product page",
		kind: Kind.Internal,
		p50: 6,
		attrs: () => ({ "template.name": "product.hbs" }),
	},
];

// Duration of a node covers its children, which are laid out back to back.
function layout(nodes, atMs, slow) {
	const times = nodes.map(() => ({ start: 0, end: 0 }));
	const kids = nodes.map((_, i) =>
		nodes.flatMap((n, j) => (n.parent === i ? [j] : [])),
	);
	const place = (i, cursor) => {
		let t = cursor;
		for (const c of kids[i]) t = place(c, t);
		const own = latency(nodes[i].p50) * slow;
		const starts = kids[i].map((c) => times[c].start);
		const ends = kids[i].map((c) => times[c].end);
		times[i].start = starts.length ? Math.min(...starts) - own * 0.35 : t;
		times[i].end =
			Math.max(
				ends.length ? Math.max(...ends) : times[i].start,
				times[i].start,
			) +
			own * 0.65;
		return times[i].end + rand(0.2, 1.5);
	};
	place(0, atMs);
	return times;
}

function makeTrace(atMs) {
	const isBrowse = Math.random() < 0.35;
	const nodes = isBrowse ? browse : checkout;
	const times = layout(nodes, atMs, Math.random() < 0.05 ? rand(2.5, 6) : 1);
	const traceId = randomBytes(16);
	const ids = nodes.map(() => randomBytes(8));
	const failed = Math.random() < 0.07;
	const failAt = isBrowse ? 3 : 13;

	const spans = nodes.map((n, i) => {
		const err = failed && i === failAt;
		const startNs = Math.round(times[i].start * 1e6);
		const endNs = Math.max(Math.round(times[i].end * 1e6), startNs + 1_000_000);
		return {
			svc: n.svc,
			body: span({
				traceId,
				spanId: ids[i],
				parentId: n.parent === undefined ? undefined : ids[n.parent],
				name: n.name,
				kind: n.kind,
				startNs,
				endNs,
				attributes: {
					...n.attrs(),
					...(err
						? {
								"http.response.status_code": 402,
								"error.type": "card_declined",
							}
						: {}),
				},
				statusCode: err || (failed && i === 0) ? 2 : 0,
				statusMessage: err
					? "card_declined"
					: failed && i === 0
						? "checkout failed"
						: "",
				events: err
					? [
							{
								timeNs: endNs,
								name: "exception",
								attributes: {
									"exception.type": "PaymentDeclined",
									"exception.message": "insufficient funds",
								},
							},
						]
					: n.name === "GetCart"
						? [
								{
									timeNs: startNs,
									name: "cache.miss",
									attributes: { key: "cart:8f21c4e0" },
								},
							]
						: [],
			}),
		};
	});

	// async receipt worker, kicked off shortly after checkout completes
	if (!isBrowse) {
		const startNs =
			Math.round(times[1].end * 1e6) + Math.round(rand(20, 90) * 1e6);
		spans.push({
			svc: S.email,
			body: span({
				traceId,
				spanId: randomBytes(8),
				parentId: ids[1],
				name: "send receipt",
				kind: Kind.Consumer,
				startNs,
				endNs: startNs + Math.round(rand(8, 60) * 1e6),
				attributes: {
					"messaging.system": "kafka",
					"messaging.destination.name": "receipts",
					"email.template": "receipt-v3",
				},
			}),
		});
	}
	return { spans, failed, isBrowse };
}

// --- traffic curve: a quiet day with lunch and evening peaks ---

const weight = (ms) => {
	const d = new Date(ms);
	const h = d.getUTCHours() + d.getUTCMinutes() / 60;
	const lunch = Math.exp(-((h - 12.5) ** 2) / 3);
	const evening = Math.exp(-((h - 18.5) ** 2) / 5);
	return 0.22 + lunch + 0.9 * evening;
};

const atMs = [];
while (atMs.length < total) {
	const t = rand(startMs, nowMs);
	if (Math.random() < weight(t) / 1.15) atMs.push(t);
}
atMs.sort((a, b) => a - b);

// --- POST traces in batches, grouped by the owning service resource ---

async function post(path, body) {
	for (let attempt = 0; attempt < 6; attempt++) {
		const res = await fetch(`${endpoint}${path}`, {
			method: "POST",
			headers: { "content-type": "application/x-protobuf" },
			body,
		});
		if (res.ok) return;
		if (res.status !== 429)
			throw new Error(`${path}: ${res.status} ${await res.text()}`);
		await new Promise((r) => setTimeout(r, 250 * (attempt + 1)));
	}
	throw new Error(`${path}: queue stayed full`);
}

const streams = {
	[S.web]: [],
	[S.gateway]: [],
	[S.cart]: [],
	[S.pricing]: [],
	[S.inventory]: [],
	[S.payment]: [],
	[S.search]: [],
	[S.email]: [],
};
const traces = atMs.map(makeTrace);
for (let i = 0; i < traces.length; i += 40) {
	const chunk = traces.slice(i, i + 40);
	for (const t of chunk) {
		for (const s of t.spans) streams[s.svc].push(s.body);
	}
	const batches = Object.entries(streams)
		.filter(([, spans]) => spans.length > 0)
		.map(([svc, spans]) => resourceSpans(resources[svc], spans));
	await post("/v1/traces", traceRequest(batches));
	for (const svc of Object.keys(streams)) streams[svc] = [];
}

// --- metrics: backdated history for every service ---

const step = 30_000;
const steps = Math.floor((nowMs - startMs) / step);
const withSvc = (svc, extra = {}) => ({ "service.name": svc, ...extra });

const baseLatency = {
	[S.web]: 55,
	[S.gateway]: 44,
	[S.cart]: 8,
	[S.pricing]: 12,
	[S.inventory]: 10,
	[S.payment]: 24,
	[S.search]: 19,
	[S.email]: 30,
};

for (const name of Object.values(S)) {
	const requests = [];
	const durations = [];
	const heap = [];
	const conns = [];
	const declines = [];
	let counter = 0;
	let heapNow = rand(220, 340);
	let connNow = rand(18, 42);
	for (let i = 0; i < steps; i++) {
		const t = startMs + i * step;
		counter +=
			Math.max(0, (weight(t) / 1.15) * rand(1.5, 4) * rand(0.6, 1.4)) | 0;
		requests.push({
			attrs: withSvc(name),
			timeNs: t * 1e6,
			startNs: startMs * 1e6,
			value: counter,
		});
		durations.push({
			attrs: withSvc(name),
			timeNs: t * 1e6,
			value: Math.max(1, baseLatency[name] * Math.exp(gauss() * 0.35)),
		});
		heapNow = Math.max(120, heapNow + rand(-6, 7));
		heap.push({
			attrs: withSvc(name),
			timeNs: t * 1e6,
			value: Math.round(heapNow * 1e6),
		});
		connNow = Math.max(2, Math.min(200, connNow + rand(-2.4, 2.2)));
		conns.push({
			attrs: withSvc(name),
			timeNs: t * 1e6,
			value: Math.round(connNow),
		});
		if (name === S.payment) {
			declines.push({
				attrs: withSvc(name, { "error.type": "card_declined" }),
				timeNs: t * 1e6,
				startNs: startMs * 1e6,
				value: Math.floor(counter * 0.07),
			});
		}
	}
	await post(
		"/v1/metrics",
		metricsRequest([
			{
				attrs: resources[name],
				metrics: [
					metric("http.server.request.count", "sum", requests, {
						unit: "1",
						description: "Requests handled",
					}),
					metric("http.server.request.duration", "gauge", durations, {
						unit: "ms",
						description: "Mean latency",
					}),
					metric("process.runtime.memory.used", "gauge", heap, {
						unit: "By",
						description: "Resident heap",
					}),
					metric("http.client.connection.count", "gauge", conns, {
						unit: "1",
						description: "Open outbound connections",
					}),
					...(name === S.payment
						? [
								metric("http.server.error.count", "sum", declines, {
									unit: "1",
									description: "Declined charges",
								}),
							]
						: []),
				],
			},
		]),
	);
}

console.log(
	`seeded ${traces.length} traces (${traces.filter((t) => t.failed).length} failed, ${traces.filter((t) => t.isBrowse).length} browse) and ${steps} metric steps per service`,
);
