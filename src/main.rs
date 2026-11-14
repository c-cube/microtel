use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use microtel::{config, ingest, pipeline, purge, store};

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
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    let cfg = config::load(cli.config, cli.listen, cli.storage_dir, cli.ttl_days)?;
    tokio::runtime::Runtime::new()?.block_on(tokio_run(cfg))
}

async fn tokio_run(cfg: config::Config) -> Result<(), Box<dyn std::error::Error>> {
    let store = Arc::new(store::Store::open(&cfg.storage_dir)?);
    let (traces, trace_rx, trace_queued) = pipeline::Pipeline::channel(cfg.queue.capacity);
    let (metrics, metric_rx, metric_queued) = pipeline::Pipeline::channel(cfg.queue.capacity);
    let app = Arc::new(ingest::App { traces, metrics });
    let trace_writer = tokio::spawn(pipeline::writer(
        trace_rx,
        store.clone(),
        cfg.queue.clone(),
        trace_queued,
    ));
    let metric_writer = tokio::spawn(pipeline::writer(
        metric_rx,
        store.clone(),
        cfg.queue,
        metric_queued,
    ));
    let purge_handle = tokio::spawn(purge::purge_task(
        store.clone(),
        cfg.ttl_days,
        cfg.purge_interval_secs,
    ));
    let listener = tokio::net::TcpListener::bind(&cfg.listen).await?;
    tracing::info!("listening on {}", listener.local_addr()?);
    tracing::info!("storage: {}", cfg.storage_dir.display());
    axum::serve(listener, ingest::router(app.clone()))
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
