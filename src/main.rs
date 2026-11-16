use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use clap::{Parser, Subcommand};
use microtel::{api, config, ingest, logging, pipeline, purge, stats, store};

#[global_allocator]
static ALLOC: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[derive(Parser)]
struct Cli {
    /// Path to YAML config (default: microtel.yaml if present)
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override listen address
    #[arg(long)]
    listen: Option<String>,
    /// Override storage directory
    #[arg(long)]
    storage_dir: Option<PathBuf>,
    /// Override retention TTL in days
    #[arg(long)]
    ttl_days: Option<u32>,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Exit 0 iff a running microtel's /health endpoint answers HTTP 200.
    Healthcheck {
        /// host:port to probe
        #[arg(long, default_value = "127.0.0.1:4318")]
        addr: String,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(Cmd::Healthcheck { addr }) = cli.cmd {
        std::process::exit(healthcheck(&addr));
    }
    logging::init();
    let cfg = config::load(cli.config, cli.listen, cli.storage_dir, cli.ttl_days)?;
    tokio::runtime::Runtime::new()?.block_on(tokio_run(cfg))
}

async fn tokio_run(cfg: config::Config) -> Result<()> {
    let store = Arc::new(store::Store::open(&cfg.storage_dir)?);
    let stats = Arc::new(stats::Stats::new());
    let (traces, trace_rx) =
        pipeline::Pipeline::channel(cfg.queue.capacity, stats.gauge(stats::Signal::Traces));
    let (metrics, metric_rx) =
        pipeline::Pipeline::channel(cfg.queue.capacity, stats.gauge(stats::Signal::Metrics));
    let app = Arc::new(ingest::App {
        traces,
        metrics,
        stats: stats.clone(),
        store: store.clone(),
    });
    let trace_writer = tokio::spawn(pipeline::writer(
        trace_rx,
        store.clone(),
        cfg.queue.clone(),
        stats.gauge(stats::Signal::Traces),
        stats.clone(),
    ));
    let metric_writer = tokio::spawn(pipeline::writer(
        metric_rx,
        store.clone(),
        cfg.queue,
        stats.gauge(stats::Signal::Metrics),
        stats.clone(),
    ));
    let purge_handle = tokio::spawn(purge::purge_task(
        store.clone(),
        cfg.ttl_days,
        cfg.purge_interval_secs,
    ));
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    log::info!("listening on {}", listener.local_addr()?);
    log::info!("storage: {}", cfg.storage_dir.display());
    axum::serve(
        listener,
        ingest::router(app.clone()).merge(api::router(store.clone())),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await?;
    drop(app);
    trace_writer.await?;
    metric_writer.await?;
    purge_handle.abort();
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("signal handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

fn healthcheck(addr: &str) -> i32 {
    let timeout = Duration::from_secs(2);
    let Ok(mut stream) = TcpStream::connect(addr) else {
        eprintln!("healthcheck: connect {addr} failed");
        return 1;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || stream.set_write_timeout(Some(timeout)).is_err()
    {
        eprintln!("healthcheck: socket setup failed");
        return 1;
    }
    let req = format!("GET /health HTTP/1.0\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    if stream.write_all(req.as_bytes()).is_err() {
        eprintln!("healthcheck: write failed");
        return 1;
    }
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).is_err() {
        eprintln!("healthcheck: read failed");
        return 1;
    }
    let ok = buf.starts_with(b"HTTP/") && buf.windows(5).any(|w| w == b" 200 ");
    if !ok {
        eprintln!("healthcheck: no HTTP 200 from {addr}");
    }
    i32::from(!ok)
}
