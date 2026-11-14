FROM rust:1-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends clang libclang-dev librocksdb-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY .cargo .cargo
COPY Cargo.toml Cargo.lock build.rs ./
COPY src src
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
