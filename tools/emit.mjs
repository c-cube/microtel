// Emits OTLP traces and metrics to a running microtel over HTTP/protobuf.
// Env: EMIT_ENDPOINT (base url), EMIT_A_TRACES, EMIT_B_TRACES.
import {
	context,
	DiagConsoleLogger,
	DiagLogLevel,
	diag,
	SpanKind,
	SpanStatusCode,
	trace,
} from "@opentelemetry/api";

if (process.env.EMIT_DIAG) {
	diag.setLogger(new DiagConsoleLogger(), DiagLogLevel.DEBUG);
}

import { OTLPMetricExporter } from "@opentelemetry/exporter-metrics-otlp-proto";
import { OTLPTraceExporter } from "@opentelemetry/exporter-trace-otlp-proto";
import { resourceFromAttributes } from "@opentelemetry/resources";
import {
	AggregationTemporality,
	MeterProvider,
	PeriodicExportingMetricReader,
} from "@opentelemetry/sdk-metrics";
import {
	BasicTracerProvider,
	BatchSpanProcessor,
} from "@opentelemetry/sdk-trace-base";

const endpoint = process.env.EMIT_ENDPOINT ?? "http://127.0.0.1:4318";
const aTraces = Number(process.env.EMIT_A_TRACES ?? 6);
const bTraces = Number(process.env.EMIT_B_TRACES ?? 4);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const providerA = new BasicTracerProvider({
	resource: resourceFromAttributes({
		"service.name": "emitter-a",
		"deployment.environment": "dev",
	}),
	spanProcessors: [
		new BatchSpanProcessor(
			new OTLPTraceExporter({
				url: `${endpoint}/v1/traces`,
				compression: "gzip",
			}),
			{ scheduledDelayMillis: 200 },
		),
	],
});
const providerB = new BasicTracerProvider({
	resource: resourceFromAttributes({ "service.name": "emitter-b" }),
	spanProcessors: [
		new BatchSpanProcessor(
			new OTLPTraceExporter({ url: `${endpoint}/v1/traces` }),
			{
				scheduledDelayMillis: 200,
			},
		),
	],
});

const tracerA = providerA.getTracer("emitter-a");
const tracerB = providerB.getTracer("emitter-b");

async function traces(tracer, label, count, spansPerTrace) {
	for (let i = 0; i < count; i++) {
		const root = tracer.startSpan(`root-${label}-${i % 4}`, {
			kind: SpanKind.SERVER,
			attributes: { "root.seq": i },
		});
		const ctx = trace.setSpan(context.active(), root);
		for (let j = 0; j < spansPerTrace; j++) {
			const child = tracer.startSpan(
				`child-${label}-${j}`,
				{ kind: j === 0 ? SpanKind.CLIENT : SpanKind.INTERNAL },
				ctx,
			);
			if (j === 1) child.addEvent("cache-miss", { key: `k-${i}` });
			if (j === spansPerTrace - 1 && i % 3 === 0) {
				child.recordException(new Error("boom"));
				child.setStatus({ code: SpanStatusCode.ERROR, message: "boom" });
			}
			await sleep(3);
			child.end();
		}
		await sleep(3);
		root.end();
	}
}

const meterProviderA = new MeterProvider({
	resource: resourceFromAttributes({ "service.name": "emitter-a" }),
	readers: [
		new PeriodicExportingMetricReader({
			exporter: new OTLPMetricExporter({
				url: `${endpoint}/v1/metrics`,
				compression: "gzip",
			}),
			exportIntervalMillis: 200,
			temporalityPreference: AggregationTemporality.CUMULATIVE,
		}),
	],
});

const meterProviderB = new MeterProvider({
	resource: resourceFromAttributes({ "service.name": "emitter-b" }),
	readers: [
		new PeriodicExportingMetricReader({
			exporter: new OTLPMetricExporter({ url: `${endpoint}/v1/metrics` }),
			exportIntervalMillis: 200,
			temporalityPreference: AggregationTemporality.DELTA,
		}),
	],
});

const requests = meterProviderA
	.getMeter("emitter-a")
	.createCounter("requests_total", { unit: "1" });
const depth = meterProviderA
	.getMeter("emitter-a")
	.createObservableGauge("queue_depth", { unit: "1" });
let observed = 10;
depth.addCallback((obs) => {
	observed += 3;
	obs.observe(observed);
});
const errors = meterProviderB
	.getMeter("emitter-b")
	.createCounter("errors_total", { unit: "1" });

const traceWork = Promise.all([
	traces(tracerA, "a", aTraces, 4),
	traces(tracerB, "b", bTraces, 3),
]);

for (let i = 0; i < 5; i++) {
	requests.add(1, { route: `/r-${i % 2}` });
	errors.add(1);
	await sleep(150);
}

await traceWork;
await providerA.shutdown();
await providerB.shutdown();
await meterProviderA.shutdown();
await meterProviderB.shutdown();
console.log("emitted");
