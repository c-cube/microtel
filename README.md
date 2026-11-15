# microtel

Tiny OpenTelemetry receiver: OTLP over HTTP + protobuf on `:4318`, stored in RocksDB
(statically linked system lib, zstd). No gRPC, no logs endpoint (501), no auth — intended
to run on localhost/tailscale.

Storage layout: spans keyed `date/trace_id/span_id` with a `date/start_ns/trace_id`
secondary index whose span-id lists are maintained by a RocksDB merge operator,
resources/scopes content-addressed by BLAKE3, metrics as `name/attr_hash/date/time_ns`
points with a `name/attr_hash` series table. Retention: TTL purge (default 30 days, runs
daily). Each signal has its own bounded queue (watermark or 1s flush).

## Run

    cp microtel.example.yaml microtel.yaml   # optional; defaults are built in
    cargo run -- --storage-dir ./data --listen 127.0.0.1:4318

CLI flags (`--config`, `--listen`, `--storage-dir`, `--ttl-days`) override the YAML.

## Test

    cargo test

Covers: key encoding, store semantics (idempotent re-export, merge, purge, content
addressing, cross-batch merge), pipeline (watermark/interval flush, drain, 429
saturation, per-signal independence), config
precedence and failure modes, and end-to-end ingest (real server + node OTLP SDK emitter,
protocol edges, SIGKILL crash recovery, backpressure). The emitter tests need node + pnpm.

## Docker

    docker build -t microtel .
    docker run --rm -p 4318:4318 -v "$PWD/data:/data" microtel --storage-dir /data --listen 0.0.0.0:4318

Builder links against the distro's static `librocksdb.a` (`librocksdb-dev` + its
`libsnappy-dev`/`libzstd-dev`/`liblz4-dev`/`libbz2-dev` deps) to skip the vendored C++
build. The image sets the `ROCKSDB_*`/`SNAPPY_*` ENV itself, so it needs no
cargo config. Local builds are another matter: `librocksdb-sys` picks system vs
vendored libraries while *it* is built, so a `build.rs` cannot influence that
choice — it has to be set in `.cargo/config.toml` before cargo starts. That file
is machine-local and gitignored; without it cargo falls back to the vendored C++
build (slow, needs clang). To use the distro rocksdb on Debian/Ubuntu amd64:

    mkdir -p .cargo && cat > .cargo/config.toml <<'EOF'
    # librocksdb-sys reads these while it is built, so they must be set before cargo
    # starts: build.rs cannot influence its vendored-vs-system choice.
    [env]
    ROCKSDB_LIB_DIR = "/usr/lib/x86_64-linux-gnu"
    ROCKSDB_INCLUDE_DIR = "/usr/include"
    ROCKSDB_STATIC = "1"
    SNAPPY_LIB_DIR = "/usr/lib/x86_64-linux-gnu"
    SNAPPY_STATIC = "1"
    EOF

`build.rs` only adds the compression archives to the final link (via `pkg-config`
libdir, falling back to `ROCKSDB_LIB_DIR`). Note the `[env]` block is part of
cargo's build fingerprint: changing it dirties `librocksdb-sys`, deleting it
silently switches to the vendored build.
