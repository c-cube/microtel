use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub storage_dir: PathBuf,
    /// OTLP ingestion endpoint.
    pub listen: String,
    /// UI/API endpoint; `None` disables the UI (CLI-only, via --no-ui).
    #[serde(default = "default_ui")]
    pub ui: Option<Ui>,
    pub ttl_days: u32,
    pub queue: Queue,
    pub purge_interval_secs: u64,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub listen: String,
}

/// Per-signal queue settings (traces and metrics each get their own queue).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Queue {
    pub capacity: usize,
    pub watermark: usize,
    pub flush_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            storage_dir: PathBuf::from("./data"),
            listen: "127.0.0.1:4318".into(),
            ui: default_ui(),
            ttl_days: 30,
            queue: Queue::default(),
            purge_interval_secs: 86_400,
        }
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            listen: "127.0.0.1:8096".into(),
        }
    }
}

fn default_ui() -> Option<Ui> {
    Some(Ui::default())
}

impl Default for Queue {
    fn default() -> Self {
        Queue {
            capacity: 1000,
            watermark: 500,
            flush_ms: 1000,
        }
    }
}

pub fn load(
    config_path: Option<PathBuf>,
    listen: Option<String>,
    ui_listen: Option<String>,
    no_ui: bool,
    storage_dir: Option<PathBuf>,
    ttl_days: Option<u32>,
) -> anyhow::Result<Config> {
    let explicit = config_path.is_some();
    let path = config_path.unwrap_or_else(|| PathBuf::from("microtel.yaml"));
    let mut cfg = if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        serde_norway::from_str::<Config>(&raw)
            .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?
    } else {
        if explicit {
            anyhow::bail!("config file not found: {}", path.display());
        }
        Config::default()
    };
    if let Some(v) = listen {
        cfg.listen = v;
    }
    if let Some(v) = ui_listen {
        cfg.ui = Some(Ui { listen: v });
    }
    if no_ui {
        cfg.ui = None;
    }
    if let Some(v) = storage_dir {
        cfg.storage_dir = v;
    }
    if let Some(v) = ttl_days {
        cfg.ttl_days = v;
    }
    Ok(cfg)
}
