//! Records which Beacon and engine commits this build came from, for About and gosub://home.
//!
//! Same rules as swift/package.sh: the short SHA (10), `-dirty` when tracked files changed,
//! `unknown` outside a git checkout. `BEACON_COMMIT` / `BEACON_ENGINE_COMMIT` in the
//! environment win, for a build from a tree without `.git` (the flatpak copies none).

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.join("../..");
    let engine = engine_dir(&root);

    let beacon = identity("BEACON_COMMIT", &root);
    let engine = engine.map_or_else(|| "unknown".to_string(), |dir| identity("BEACON_ENGINE_COMMIT", &dir));
    println!("cargo:rustc-env=BEACON_COMMIT={beacon}");
    println!("cargo:rustc-env=BEACON_ENGINE_COMMIT={engine}");
}

/// The engine checkout, from the workspace's `gosub_engine` path dependency.
fn engine_dir(root: &Path) -> Option<PathBuf> {
    let toml = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let line = toml.lines().find(|l| l.starts_with("gosub_engine = "))?;
    let path = line.split("path = \"").nth(1)?.split('"').next()?;
    let crate_dir = root.join(path);
    Some(crate_dir.join("../.."))
}

fn identity(var: &str, dir: &Path) -> String {
    println!("cargo:rerun-if-env-changed={var}");
    if let Ok(value) = std::env::var(var) {
        if !value.is_empty() {
            return value;
        }
    }
    let Some(sha) = git(dir, &["rev-parse", "--short=10", "HEAD"]) else {
        return "unknown".to_string();
    };
    // In a worktree .git is a file, so ask git where HEAD, the index and the refs live.
    for path in ["HEAD", "index", "packed-refs"] {
        if let Some(p) = git(dir, &["rev-parse", "--git-path", path]) {
            println!("cargo:rerun-if-changed={}", dir.join(p).display());
        }
    }
    if let Some(head_ref) = git(dir, &["symbolic-ref", "-q", "HEAD"]) {
        if let Some(p) = git(dir, &["rev-parse", "--git-path", &head_ref]) {
            println!("cargo:rerun-if-changed={}", dir.join(p).display());
        }
    }
    let dirty = git(dir, &["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    if dirty {
        format!("{sha}-dirty")
    } else {
        sha
    }
}

/// Trimmed stdout of a git command run in `dir`, or `None` when it fails.
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
