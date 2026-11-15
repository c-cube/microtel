FROM node:22-slim AS ui
RUN corepack enable
WORKDIR /ui
COPY ui/package.json ui/pnpm-lock.yaml ui/pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY ui .
RUN pnpm build

FROM rust:1-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libclang-dev librocksdb-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# librocksdb-sys reads these while it is built, so they must be plain ENV: a
# build.rs cannot influence its vendored-vs-system choice.
ENV ROCKSDB_LIB_DIR=/usr/lib/x86_64-linux-gnu \
    ROCKSDB_INCLUDE_DIR=/usr/include \
    ROCKSDB_STATIC=1 \
    SNAPPY_LIB_DIR=/usr/lib/x86_64-linux-gnu \
    SNAPPY_STATIC=1
COPY Cargo.toml Cargo.lock build.rs ./
COPY src src
COPY --from=ui /ui/dist ui/dist
RUN cargo build --release && strip target/release/microtel

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends libstdc++6 \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/microtel /usr/local/bin/microtel
EXPOSE 4318
VOLUME /data
ENTRYPOINT ["microtel"]
CMD ["--listen", "0.0.0.0:4318", "--storage-dir", "/data"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=5s --retries=3 \
    CMD ["microtel", "healthcheck"]
