
# microtel

There are many opentelemetry tools, but this one is mine. I just want something simple
for my VPS, that will consume very little resources, so here we are:
- single rust binary
- rocksdb for storage, no separate DB
- listens for HTTP + protobuf spans and metrics (logs are not supported for now)
- serves a basic API + solidjs UI (pure vibes ✨)

That's it. It seems to be fine for me and runs in a few dozen MBs of memory.

## Scope

Truly this is for my 5$ VPS and for local development. Feel free to fork (MIT licensed) and do whatever, but the scope is intentionally very small, and this will mostly evolve to serve my specific needs.

## web UI

![Traces](media/traces.png)

![Metrics](media/metrics.png)
