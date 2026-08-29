//! Crash-safe persistence for the state files under `~/.config/marrow`
//! (issue #200).
//!
//! Every state write goes through `write_atomic`: content lands in a temp
//! file in the same directory (created private from the first byte), is
//! synced, and is renamed over the destination. Readers — including a second
//! app instance sharing the config dir — see the old file or the new file,
//! never a torn hybrid, and a crash mid-write can no longer silently erase
//! state (all loaders treat unparseable as absent).
//!
//! `with_lock` adds an advisory exclusive lock for updates that span multiple
//! files (the manifest + metadata sidecar pair).
//!
//! Non-goal, on purpose: semantic merging of concurrent read-modify-write
//! cycles across processes. The frontend and the resolve scripts hold
//! whole-value state; last-COMPLETE-writer-wins is the intended semantic for
//! a single-user tool. This module upgrades that from "last writer wins,
//! torn files possible" to "last complete write wins, always parseable".

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Atomically replace `path` with `bytes`. The temp file lives in the same
/// directory (rename must not cross filesystems) and is created `0o600` on
/// unix from the first byte — state files carry tokens and private code, so
/// there is no world-readable window. The parent directory is created if
/// missing.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("No parent directory for {}", path.display()))?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;

    // pid + per-process counter + nanos: unique even for two unlocked writes
    // to the same path in the same nanosecond (create_new would otherwise
    // fail one of them with "File exists").
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let base = path
        .file_name()
        .ok_or_else(|| format!("No file name in {}", path.display()))?
        .to_string_lossy()
        .into_owned();
    let tmp = parent.join(format!("{}.tmp-{}-{}-{}", base, std::process::id(), seq, nanos));

    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }

    let result = (|| {
        let mut f = opts
            .open(&tmp)
            .map_err(|e| format!("Failed to create temp file {}: {}", tmp.display(), e))?;
        f.write_all(bytes)
            .map_err(|e| format!("Failed to write {}: {}", tmp.display(), e))?;
        // Durability: the rename below must not land before the content does.
        f.sync_all()
            .map_err(|e| format!("Failed to sync {}: {}", tmp.display(), e))?;
        drop(f);
        // Windows (CLI builds) can't rename over an existing file.
        #[cfg(windows)]
        let _ = fs::remove_file(path);
        fs::rename(&tmp, path)
            .map_err(|e| format!("Failed to replace {}: {}", path.display(), e))?;
        // Best-effort: sync the directory entry too, so a crash right after
        // can't lose the rename itself. Failure here costs durability of
        // this one write, never atomicity — the prior file stays parseable.
        #[cfg(unix)]
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Run `f` while holding an advisory exclusive lock on `lock_path` (created
/// if missing). Used where one logical update spans multiple files, so
/// concurrent writers serialize instead of interleaving. Advisory only:
/// writers that don't take the lock aren't blocked — pair it with
/// `write_atomic` so even those can't tear a single file.
pub fn with_lock<T>(lock_path: &Path, f: impl FnOnce() -> T) -> Result<T, String> {
    use fs2::FileExt;
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
    }
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_path)
        .map_err(|e| format!("Failed to open lock {}: {}", lock_path.display(), e))?;
    lock.lock_exclusive()
        .map_err(|e| format!("Failed to lock {}: {}", lock_path.display(), e))?;
    let out = f();
    let _ = fs2::FileExt::unlock(&lock);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "marrow-stateio-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn write_atomic_creates_and_replaces() {
        let d = tmp_dir("replace");
        let p = d.join("state.json");
        write_atomic(&p, b"{\"v\":1}").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\"v\":1}");
        write_atomic(&p, b"{\"v\":2}").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "{\"v\":2}");
        // No temp litter left behind.
        let leftovers: Vec<_> = fs::read_dir(&d)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files should be renamed away");
        let _ = fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_is_private_from_creation() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmp_dir("perms");
        let p = d.join("config");
        write_atomic(&p, b"github_token=secret").unwrap();
        let mode = fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "state files must be owner-only");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn write_atomic_creates_missing_parent() {
        let d = tmp_dir("parent");
        let p = d.join("nested").join("deep").join("s.json");
        write_atomic(&p, b"x").unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "x");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn every_state_module_writes_through_write_atomic() {
        // Regression lint: the point of issue #200 is that NO state module
        // does a raw fs::write. Checks each migrated module's non-test code;
        // a new raw write (or a new state module added to this list) must go
        // through write_atomic.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let modules = [
            "config.rs", "session.rs", "manifest_cache.rs", "dismissed_highlights.rs",
            "resolved_specs.rs", "viewed_state.rs", "chat_history.rs", "watches.rs",
            "pr_requirements.rs", "checks_dismiss.rs", "activity.rs",
        ];
        for m in modules {
            let text = fs::read_to_string(src.join(m)).unwrap();
            let non_test = text.split("#[cfg(test)]").next().unwrap();
            assert!(
                !non_test.contains("fs::write("),
                "{m} has a raw fs::write outside tests — use state_io::write_atomic"
            );
        }
    }

    #[test]
    fn locked_pair_writes_never_cross() {
        // The manifest-cache pattern: one logical update = two files that
        // must correspond. Concurrent writers under with_lock + write_atomic
        // must leave a matching pair — never A's first file with B's second.
        let d = tmp_dir("pair");
        let (main, side, lock) = (d.join("m.json"), d.join("m.meta.json"), d.join("m.lock"));
        let mut handles = Vec::new();
        for tag in ["A", "B", "C", "D"] {
            let (main, side, lock) = (main.clone(), side.clone(), lock.clone());
            handles.push(std::thread::spawn(move || {
                for i in 0..25 {
                    let v = format!("{}{}", tag, i);
                    with_lock(&lock, || -> Result<(), String> {
                        write_atomic(&main, v.as_bytes())?;
                        // Widen the race window between the pair's writes.
                        std::thread::sleep(std::time::Duration::from_micros(200));
                        write_atomic(&side, v.as_bytes())
                    })
                    .unwrap()
                    .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let (m, s) = (fs::read_to_string(&main).unwrap(), fs::read_to_string(&side).unwrap());
        assert_eq!(m, s, "pair must correspond after concurrent writers");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn with_lock_serializes_concurrent_writers() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        let d = tmp_dir("lock");
        let lock = d.join("pair.lock");
        let inside = Arc::new(AtomicU32::new(0));
        let peak = Arc::new(AtomicU32::new(0));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let (lock, inside, peak) = (lock.clone(), inside.clone(), peak.clone());
            handles.push(std::thread::spawn(move || {
                with_lock(&lock, || {
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(5));
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
                .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1, "critical section must be exclusive");
        let _ = fs::remove_dir_all(&d);
    }
}
