#!/usr/bin/env python3
"""Send a deterministic-shape, synthetic 24h OTLP dataset to local microtel.

Usage:
    python3 tools/mockdata.py
    MOCKTEL_ENDPOINT=http://127.0.0.1:4319 MOCKTEL_TRACES=800 python3 tools/mockdata.py

Only Python's standard library is required. The receiver persists the data in
./mockdata/ when started with --storage-dir ./mockdata/rocksdb.
"""

import math
import os
import random
import struct
import time
import urllib.error
import urllib.request
import uuid


ENDPOINT = os.getenv("MOCKTEL_ENDPOINT", "http://127.0.0.1:4319").rstrip("/")
HOURS = int(os.getenv("MOCKTEL_HOURS", "24"))
TRACE_COUNT = int(os.getenv("MOCKTEL_TRACES", "600"))
random.seed(2601)


def varint(value):
    value = int(value)
    out = bytearray()
    while value > 0x7F:
        out.append((value & 0x7F) | 0x80)
        value >>= 7
    out.append(value)
    return bytes(out)


def tag(field, wire):
    return varint((field << 3) | wire)


def blob(field, value):
    value = bytes(value)
    return tag(field, 2) + varint(len(value)) + value


def text(field, value):
    return blob(field, str(value).encode())


def uint(field, value):
    return tag(field, 0) + varint(value)


def fixed64(field, value):
    return tag(field, 1) + struct.pack("<Q", int(value))


def double(field, value):
    return tag(field, 1) + struct.pack("<d", float(value))


def any_value(value):
    if isinstance(value, bool):
        return uint(2, int(value))
    if isinstance(value, int):
        return uint(3, value)
    if isinstance(value, float):
        return double(4, value)
    return text(1, value)


def kv(key, value):
    return text(1, key) + blob(2, any_value(value))


def attributes(values):
    return b"".join(blob(1, kv(k, v)) for k, v in values.items())


def resource(attrs):
    return attributes(attrs)


def scope(name="microtel-python-demo"):
    return text(1, name) + text(2, "1.0")


def encode_span(trace_id, span_id, parent_id, name, kind, start_ns, end_ns,
                attrs, error=False):
    out = (
        blob(1, trace_id) + blob(2, span_id)
        + (blob(4, parent_id) if parent_id else b"")
        + text(5, name) + uint(6, kind)
        + fixed64(7, start_ns) + fixed64(8, end_ns)
        + b"".join(blob(9, kv(k, v)) for k, v in attrs.items())
    )
    if error:
        status = text(2, "simulated upstream failure") + uint(3, 2)
        out += blob(15, status)
    return out


def resource_spans(service, spans):
    attrs = {
        "service.name": service,
        "service.version": "1.4.0",
        "deployment.environment": "demo",
        "host.name": f"{service}-01",
    }
    scope_spans = blob(1, scope()) + b"".join(blob(2, s) for s in spans)
    return blob(1, resource(attrs)) + blob(2, scope_spans)


def post(path, body):
    request = urllib.request.Request(
        ENDPOINT + path, data=body,
        headers={"Content-Type": "application/x-protobuf"},
        method="POST",
    )
    for attempt in range(6):
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                if response.status == 200:
                    return
                raise RuntimeError(f"{path}: HTTP {response.status}")
        except urllib.error.HTTPError as exc:
            if exc.code != 429 or attempt == 5:
                raise RuntimeError(f"{path}: HTTP {exc.code}: {exc.read()!r}") from exc
            time.sleep(0.25 * (attempt + 1))


SERVICES = {
    "frontend": {"service.name": "frontend", "service.version": "2.8.1",
                 "deployment.environment": "demo", "host.name": "web-01"},
    "catalog-api": {"service.name": "catalog-api", "service.version": "1.4.0",
                    "deployment.environment": "demo", "host.name": "api-01"},
    "postgresql": {"service.name": "postgresql", "service.version": "16",
                   "deployment.environment": "demo", "host.name": "db-01"},
    "redis": {"service.name": "redis", "service.version": "7.2",
              "deployment.environment": "demo", "host.name": "cache-01"},
}


def make_trace(at_ms):
    trace_id = uuid.uuid4().bytes
    ids = [uuid.uuid4().bytes[:8] for _ in range(8)]
    slow = random.random() < 0.06
    failed = random.random() < 0.045
    scale = random.uniform(2.8, 5.0) if slow else 1.0
    origin = int(at_ms * 1_000_000)
    # Nested browser -> API -> DB/cache work; the DB and cache spans have their
    # own service resources so the service grouping is useful in the UI.
    specs = [
        ("frontend", "GET /products", 2, None, 0, 150 * scale,
         {"http.request.method": "GET", "url.path": "/products",
          "http.response.status_code": 500 if failed else 200}),
        ("frontend", "render product page", 1, 0, 8, 38 * scale,
         {"ui.framework": "solid-js", "component": "ProductPage"}),
        ("frontend", "GET /api/products", 3, 0, 3, 112 * scale,
         {"http.request.method": "GET", "server.address": "catalog-api"}),
        ("catalog-api", "GET /products", 2, 2, 5, 105 * scale,
         {"http.request.method": "GET", "url.path": "/products",
          "http.response.status_code": 503 if failed else 200}),
        ("catalog-api", "SELECT products", 3, 3, 18, 65 * scale,
         {"db.system": "postgresql", "db.operation": "SELECT",
          "db.statement": "SELECT id, name, price FROM products WHERE id = $1"}),
        ("postgresql", "SELECT products", 2, 4, 22, 59 * scale,
         {"db.system": "postgresql", "db.operation": "SELECT",
          "db.name": "catalog"}),
        ("catalog-api", "GET product cache", 3, 3, 70, 89 * scale,
         {"db.system": "redis", "db.operation": "GET",
          "db.statement": "GET product:sku-42"}),
        ("redis", "GET product cache", 2, 6, 72, 78 * scale,
         {"db.system": "redis", "db.operation": "GET"}),
    ]
    by_service = {service: [] for service in SERVICES}
    for index, (service, name, kind, parent, start_ms, end_ms, attrs) in enumerate(specs):
        if failed and index in (0, 3):
            attrs = {**attrs, "error.type": "upstream_unavailable"}
        by_service[service].append(encode_span(
            trace_id, ids[index], ids[parent] if parent is not None else None,
            name, kind, origin + int(start_ms * 1e6),
            origin + int(end_ms * 1e6), attrs, failed and index in (0, 3),
        ))
    return by_service


def send_traces():
    now_ms = int(time.time() * 1000) - 5_000
    from_ms = now_ms - HOURS * 3_600_000
    # Day/night traffic variation keeps the time chart from looking uniform.
    times = []
    while len(times) < TRACE_COUNT:
        t = random.uniform(from_ms, now_ms)
        hour = (t / 3_600_000) % 24
        weight = 0.2 + math.exp(-((hour - 13) ** 2) / 8) + 0.8 * math.exp(-((hour - 19) ** 2) / 10)
        if random.random() < weight / 1.15:
            times.append(t)
    times.sort()
    for base in range(0, len(times), 30):
        streams = {service: [] for service in SERVICES}
        for t in times[base:base + 30]:
            for service, spans in make_trace(t).items():
                streams[service].extend(spans)
        batches = [resource_spans(service, spans)
                   for service, spans in streams.items() if spans]
        post("/v1/traces", b"".join(blob(1, batch) for batch in batches))
    return now_ms


def metric(name, kind, points, unit, description):
    data_points = []
    for timestamp, value, attrs in points:
        dp = (fixed64(2, timestamp) + fixed64(3, timestamp)
              + double(4, value)
              + b"".join(blob(7, kv(k, v)) for k, v in attrs.items()))
        data_points.append(blob(1, dp))
    body = b"".join(data_points)
    data = (blob(7, body + uint(2, 2) + uint(3, 1))
            if kind == "sum" else blob(5, body))
    return text(1, name) + text(2, description) + text(3, unit) + data


def send_metrics(now_ms):
    from_ms = now_ms - HOURS * 3_600_000
    step_ms = 60_000
    count = HOURS * 60
    for service, res_attrs in SERVICES.items():
        request_metrics = []
        request_count = []
        latency = []
        memory = []
        connections = []
        total = 0
        mem = random.uniform(260, 520)
        for i in range(count):
            t = from_ms + i * step_ms
            hour = (t / 3_600_000) % 24
            traffic = 0.25 + math.exp(-((hour - 13) ** 2) / 8) + 0.8 * math.exp(-((hour - 19) ** 2) / 10)
            total += max(1, round(traffic * random.uniform(2, 7)))
            attrs = {"service.name": service}
            request_count.append((t * 1_000_000, total, attrs))
            base = {"frontend": 58, "catalog-api": 34, "postgresql": 12, "redis": 4}[service]
            latency.append((t * 1_000_000,
                            max(0.5, random.lognormvariate(math.log(base * traffic), 0.24)),
                            attrs))
            mem = max(120, mem + random.uniform(-9, 10))
            memory.append((t * 1_000_000, mem * 1_000_000, attrs))
            connections.append((t * 1_000_000, random.randint(4, 60), attrs))
        metrics = [
            metric("http.server.request.count", "sum", request_count, "1", "Total requests"),
            metric("http.server.request.duration", "gauge", latency, "ms", "Request latency"),
            metric("process.runtime.memory.used", "gauge", memory, "By", "Runtime memory"),
            metric("http.client.connection.count", "gauge", connections, "1", "Open connections"),
        ]
        scope_metrics = blob(1, scope()) + b"".join(blob(2, m) for m in metrics)
        resource_metrics = blob(1, resource(res_attrs)) + blob(2, scope_metrics)
        post("/v1/metrics", blob(1, resource_metrics))


if __name__ == "__main__":
    if HOURS <= 0 or TRACE_COUNT <= 0:
        raise SystemExit("MOCKTEL_HOURS and MOCKTEL_TRACES must be positive")
    try:
        with urllib.request.urlopen(ENDPOINT + "/health", timeout=3) as response:
            if response.status != 200:
                raise SystemExit(f"microtel health check returned HTTP {response.status}")
    except Exception as exc:
        raise SystemExit(f"cannot reach microtel at {ENDPOINT}: {exc}") from exc
    end_ms = send_traces()
    send_metrics(end_ms)
    print(f"sent {TRACE_COUNT} traces across {HOURS}h and {HOURS * 60} metric points/service to {ENDPOINT}")
