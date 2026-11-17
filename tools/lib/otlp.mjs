// Minimal OTLP/protobuf encoder: just enough of the wire format to mint
// traces and metrics without pulling the opentelemetry SDK into this tool.

const varint = (n) => {
	const out = [];
	let v = BigInt(n);
	do {
		let b = Number(v & 0x7fn);
		v >>= 7n;
		if (v > 0n) b |= 0x80;
		out.push(b);
	} while (v > 0n);
	return Buffer.from(out);
};

const tag = (field, wire) => varint((field << 3) | wire);

const concat = (parts) => Buffer.concat(parts);

export const bytes = (field, v) =>
	concat([tag(field, 2), varint(v.length), Buffer.from(v)]);
export const str = (field, v) => bytes(field, Buffer.from(String(v), "utf8"));
export const uint = (field, v) => concat([tag(field, 0), varint(v)]);
export const double = (field, v) => {
	const b = Buffer.alloc(8);
	b.writeDoubleLE(v);
	return concat([tag(field, 1), b]);
};
export const fixed64 = (field, v) => {
	const b = Buffer.alloc(8);
	b.writeBigUInt64LE(BigInt(Math.round(v)) & 0xffffffffffffffffn);
	return concat([tag(field, 1), b]);
};
const raw64 = (v) => {
	const b = Buffer.alloc(8);
	b.writeBigUInt64LE(BigInt(Math.max(0, Math.round(v))) & 0xffffffffffffffffn);
	return b;
};
const rawDouble = (v) => {
	const b = Buffer.alloc(8);
	b.writeDoubleLE(v);
	return b;
};
// sint32: zigzag varint
const sint = (field, v) => uint(field, v >= 0 ? 2n * BigInt(v) : -2n * BigInt(v) - 1n);

// --- common ---

const anyValue = (v) =>
	typeof v === "number"
		? Number.isInteger(v)
			? uint(3, v) // int_value
			: double(4, v) // double_value
		: str(1, v); // string_value

export const kv = (key, v) => concat([str(1, key), bytes(2, anyValue(v))]);

/** Resource message: repeated KeyValue attributes = 1 */
export const resource = (list) =>
	concat(Object.entries(list).map(([k, v]) => bytes(1, kv(k, v))));

/** InstrumentationScope */
export const scope = (name, version = "0.1.0") =>
	concat([str(1, name), str(2, version)]);

// --- traces ---

export const Kind = {
	Internal: 1,
	Server: 2,
	Client: 3,
	Producer: 4,
	Consumer: 5,
};

export const span = ({
	traceId,
	spanId,
	parentId,
	name,
	kind = Kind.Internal,
	startNs,
	endNs,
	attributes = {},
	statusCode = 0,
	statusMessage = "",
	events = [],
}) => {
	const parts = [
		bytes(1, traceId),
		bytes(2, spanId),
		parentId ? bytes(4, parentId) : Buffer.alloc(0),
		str(5, name),
		uint(6, kind),
		fixed64(7, startNs),
		fixed64(8, endNs),
	];
	for (const [k, v] of Object.entries(attributes))
		parts.push(bytes(9, kv(k, v)));
	for (const e of events) {
		const ev = concat([
			fixed64(1, e.timeNs),
			str(2, e.name),
			...Object.entries(e.attributes ?? {}).map(([k, v]) => bytes(3, kv(k, v))),
		]);
		parts.push(bytes(11, ev));
	}
	if (statusCode !== 0 || statusMessage) {
		parts.push(
			bytes(
				15,
				concat([
					statusMessage ? str(2, statusMessage) : Buffer.alloc(0),
					uint(3, statusCode),
				]),
			),
		);
	}
	return concat(parts);
};

export const resourceSpans = (resourceAttrs, spans) =>
	concat([
		bytes(1, resource(resourceAttrs)),
		bytes(
			2,
			concat([
				bytes(1, scope("microtel-seed")),
				...spans.map((s) => bytes(2, s)),
			]),
		),
	]);

export const traceRequest = (batches) =>
	concat(batches.map((b) => bytes(1, b)));

// --- metrics ---

/** kind: "gauge" | "sum"; points: [{ attrs, timeNs, startNs?, value }] */
export const metric = (
	name,
	kind,
	points,
	{ unit = "", description = "" } = {},
) => {
	let data;
	if (kind === "histogram") {
		// points: { timeNs, startNs?, count, sum?, bounds, counts, attrs? } (cumulative)
		const dps = points.map((p) =>
			bytes(
				1,
				concat([
					p.startNs != null ? fixed64(2, p.startNs) : Buffer.alloc(0),
					fixed64(3, p.timeNs),
					fixed64(4, p.count),
					p.sum != null ? double(5, p.sum) : Buffer.alloc(0),
					bytes(6, concat(p.counts.map(raw64))),
					bytes(7, concat(p.bounds.map(rawDouble))),
					...Object.entries(p.attrs ?? {}).map(([k, v]) => bytes(9, kv(k, v))),
				]),
			),
		);
		data = bytes(9, concat([concat(dps), uint(2, 2)]));
	} else if (kind === "exponential") {
		// points: { timeNs, startNs?, count, sum?, scale, zeroCount?, zeroThreshold?,
		//           positive?: { offset, counts }, negative?: { offset, counts }, attrs? } (cumulative)
		const buckets = (field, b) =>
			b
				? bytes(
						field,
						concat([sint(1, b.offset), bytes(2, concat(b.counts.map(raw64)))]),
				  )
				: Buffer.alloc(0);
		const dps = points.map((p) =>
			bytes(
				1,
				concat([
					p.startNs != null ? fixed64(2, p.startNs) : Buffer.alloc(0),
					fixed64(3, p.timeNs),
					fixed64(4, p.count),
					p.sum != null ? double(5, p.sum) : Buffer.alloc(0),
					sint(6, p.scale),
					fixed64(7, p.zeroCount ?? 0),
					buckets(8, p.positive),
					buckets(9, p.negative),
					double(14, p.zeroThreshold ?? 0),
					...Object.entries(p.attrs ?? {}).map(([k, v]) => bytes(1, kv(k, v))),
				]),
			),
		);
		data = bytes(10, concat([concat(dps), uint(2, 2)]));
	} else {
		const dps = points.map((p) =>
			bytes(
				1,
				concat([
					p.startNs != null ? fixed64(2, p.startNs) : Buffer.alloc(0),
					fixed64(3, p.timeNs),
					double(4, p.value),
					...Object.entries(p.attrs).map(([k, v]) => bytes(7, kv(k, v))),
				]),
			),
		);
		data =
			kind === "sum"
				? bytes(7, concat([body(dps), uint(2, 2), uint(3, 1)])) // cumulative, monotonic
				: bytes(5, body(dps));
	}
	return concat([
		str(1, name),
		description ? str(2, description) : Buffer.alloc(0),
		unit ? str(3, unit) : Buffer.alloc(0),
		data,
	]);
};

const body = (dps) => concat(dps);

/** Resources: [{ attrs, metrics: [metricBytes] }] */
export const metricsRequest = (resources) =>
	concat(
		resources.map(({ attrs, metrics }) =>
			bytes(
				1,
				concat([
					bytes(1, resource(attrs)),
					bytes(
						2,
						concat([
							bytes(1, scope("microtel-seed")),
							...metrics.map((m) => bytes(2, m)),
						]),
					),
				]),
			),
		),
	);
