use std::io::Write;
use std::sync::OnceLock;

use log::{LevelFilter, Log, Metadata, Record};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

static MAX: OnceLock<LevelFilter> = OnceLock::new();

fn max() -> LevelFilter {
    *MAX.get().unwrap_or(&LevelFilter::Info)
}

pub fn init() {
    let lvl = std::env::var("MICROTEL_LOG")
        .or_else(|_| std::env::var("RUST_LOG"))
        .ok()
        .and_then(|s| s.trim().parse::<LevelFilter>().ok())
        .unwrap_or(LevelFilter::Info);
    let _ = MAX.set(lvl);
    let _ = log::set_logger(&Logger);
    log::set_max_level(lvl);
}

struct Logger;

impl Log for Logger {
    fn enabled(&self, meta: &Metadata) -> bool {
        meta.level() <= max()
    }

    fn log(&self, rec: &Record) {
        if !self.enabled(rec.metadata()) {
            return;
        }
        let ts = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default();
        let _ = writeln!(std::io::stderr(), "{ts} {:<5} {}", rec.level(), rec.args());
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}
