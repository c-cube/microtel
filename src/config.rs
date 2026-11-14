use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    pub storage_dir: PathBuf,
    pub listen: String,
    pub ttl_days: u32,
    pub queue: Queue,
    pub purge_interval_secs: u64,
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
            ttl_days: 30,
            queue: Queue::default(),
            purge_interval_secs: 86_400,
        }
    }
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

type Err = Box<dyn std::error::Error>;

pub fn load(
    config_path: Option<PathBuf>,
    listen: Option<String>,
    storage_dir: Option<PathBuf>,
    ttl_days: Option<u32>,
) -> Result<Config, Err> {
    let explicit = config_path.is_some();
    let path = config_path.unwrap_or_else(|| PathBuf::from("microtel.yaml"));
    let mut cfg = if path.exists() {
        let raw = std::fs::read_to_string(&path)?;
        serde_norway::from_str::<Config>(&raw).map_err(|e| format!("{}: {e}", path.display()))?
    } else {
        if explicit {
            return Err(format!("config file not found: {}", path.display()).into());
        }
        Config::default()
    };
    if let Some(v) = listen {
        cfg.listen = v;
    }
    if let Some(v) = storage_dir {
        cfg.storage_dir = v;
    }
    if let Some(v) = ttl_days {
        cfg.ttl_days = v;
    }
    Ok(cfg)
}
