//! Link the distro compression archives the static `librocksdb.a` needs.
//! Emits nothing on the vendored path (librocksdb-sys links its own copies).

use std::path::{Path, PathBuf};
use std::process::Command;

const COMPRESSION: [&str; 5] = ["zstd", "lz4", "snappy", "bz2", "z"];

fn main() {
    println!("cargo:rerun-if-env-changed=ROCKSDB_LIB_DIR");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG");

    let Ok(from_env) = std::env::var("ROCKSDB_LIB_DIR") else {
        return;
    };
    let dir = pkg_config_libdir()
        .filter(|d| d.join("librocksdb.a").exists())
        .unwrap_or_else(|| PathBuf::from(from_env));

    // absolute paths: `-l:libfoo.a` would pick librocksdb-sys' vendored copy first
    for lib in COMPRESSION {
        if let Some(path) = find_lib(&dir, lib) {
            println!("cargo:rustc-link-arg={}", path.display());
        }
    }
}

fn pkg_config_libdir() -> Option<PathBuf> {
    let pkg_config = std::env::var("PKG_CONFIG").unwrap_or_else(|_| "pkg-config".into());
    let out = Command::new(pkg_config)
        .args(["--variable=libdir", "rocksdb"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let dir = PathBuf::from(String::from_utf8(out.stdout).ok()?.trim());
    dir.is_dir().then_some(dir)
}

fn find_lib(dir: &Path, lib: &str) -> Option<PathBuf> {
    [format!("lib{lib}.a"), format!("lib{lib}.so")]
        .iter()
        .map(|name| dir.join(name))
        .find(|path| path.exists())
}
