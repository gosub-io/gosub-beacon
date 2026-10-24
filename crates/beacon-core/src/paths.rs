//! Where Beacon keeps a profile on disk.
//!
//! The layout itself (folders, `profile.toml`, the lock, migrating the old flat layout) is
//! the `beacon-profile` crate's. This module picks the profile from the command line, once
//! per process, and hands out its folders.

use beacon_profile::crypto::{KeyStore, OsKeychain, ProfileKey, Store};
use beacon_profile::{Profile, ProfileLock, Roots, Selection};
use gosub_engine::storage::{is_plaintext_database, SqliteKey};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

static PROFILE: OnceLock<Profile> = OnceLock::new();
/// Where profile keys go in files, on a platform without a keychain; see [`keys_dir`].
static KEYS_DIR: OnceLock<PathBuf> = OnceLock::new();
static LOCK: OnceLock<ProfileLock> = OnceLock::new();
static KEY: OnceLock<ProfileKey> = OnceLock::new();
static ENCRYPTED: OnceLock<()> = OnceLock::new();

/// Resolve the profile from `--user-data-dir` / `--profile`, creating or migrating it as
/// needed. Call early in `main`, after the command line and before the lockdown, which
/// only lets this process write inside the profile and its cache. Nothing is logged here:
/// the logger does not exist yet, so the error goes back to the caller to print.
pub fn init() -> anyhow::Result<&'static Profile> {
    if let Some(profile) = PROFILE.get() {
        return Ok(profile);
    }
    let (profile, keys_dir) = resolve(crate::cli::Cli::global())?;
    let _ = KEYS_DIR.set(keys_dir);
    Ok(PROFILE.get_or_init(|| profile))
}

/// The profile `cli` names, opened, and where its key files would go (see [`keys_dir`]).
/// Publishes nothing.
fn resolve(cli: &crate::cli::Cli) -> anyhow::Result<(Profile, PathBuf)> {
    let selection = match (&cli.user_data_dir, &cli.profile) {
        (Some(dir), _) => Selection::Dir(dir.clone()),
        (None, Some(name)) => Selection::Named(name.clone()),
        (None, None) => Selection::Default,
    };
    let roots = match &selection {
        // An exact folder still needs a cache root, and a place for key files where there
        // is no keychain; without a platform data directory (Android has none through
        // `dirs`) both go next to the folder, never inside it.
        Selection::Dir(dir) => Roots::platform().unwrap_or_else(|_| {
            let beside = dir.parent().unwrap_or(dir);
            Roots {
                data: beside.join("gosub-beacon"),
                cache: beside.join("cache"),
            }
        }),
        _ => Roots::platform()?,
    };
    let profile = roots.open(&selection, env!("CARGO_PKG_VERSION"))?;
    Ok((profile, roots.data.join("keys")))
}

/// Run the profile in `dir`, for a host on the C ABI, which has no command line to give
/// `--user-data-dir` on. The profile is opened, locked and unlocked before anything is
/// published, so a folder that fails (in use, its key elsewhere) leaves the process free
/// to try another. Once one succeeds it is the process's profile: a later call accepts
/// the same folder, however it is spelled, and refuses any other.
pub fn open_folder(dir: &Path) -> anyhow::Result<()> {
    if PROFILE.get().is_some() {
        let running = crate::cli::Cli::global().user_data_dir.as_deref();
        return match running {
            Some(running) if same_folder(running, dir) => Ok(()),
            _ => anyhow::bail!(
                "{} asked for, but this process already runs the profile in {}",
                dir.display(),
                profile().dir.display()
            ),
        };
    }
    let cli = crate::cli::Cli {
        user_data_dir: Some(dir.to_path_buf()),
        ..Default::default()
    };
    let (profile, keys_dir) = resolve(&cli)?;
    let lock = profile.lock()?;
    let keys = key_store(&profile, Some(&keys_dir));
    // A copy, as in `profile_key`: unlocking may write `[crypto]` into the manifest.
    let key = profile.clone().unlock(keys.as_deref())?;
    encrypt_plaintext(&profile, &key)?;

    let published = crate::cli::Cli::set(cli);
    anyhow::ensure!(
        published.user_data_dir.as_deref() == Some(dir),
        "{} asked for, but this process has already settled on another profile",
        dir.display()
    );
    let _ = KEYS_DIR.set(keys_dir);
    let _ = PROFILE.set(profile);
    let _ = LOCK.set(lock);
    let _ = KEY.set(key);
    let _ = ENCRYPTED.set(());
    Ok(())
}

/// Whether two spellings name one folder: `./p`, `a/../p` and a symlink to it are the same
/// profile. Resolved on disk; a folder that is not there yet is compared as an absolute path.
fn same_folder(a: &Path, b: &Path) -> bool {
    let resolve = |p: &Path| {
        std::fs::canonicalize(p)
            .or_else(|_| std::path::absolute(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    resolve(a) == resolve(b)
}

/// The folder profile keys are kept in as files, when this platform keeps them that way:
/// Android, which has no keychain Beacon can use. `None` where the OS keychain holds them.
/// Outside every profile and its cache, so the lockdown has to grant it on its own.
pub fn keys_dir() -> Option<&'static Path> {
    file_keys(KEYS_DIR.get().map(PathBuf::as_path))
}

fn file_keys(dir: Option<&Path>) -> Option<&Path> {
    if OsKeychain::new().is_some() || !cfg!(target_os = "android") {
        return None;
    }
    dir
}

/// Where this machine keeps `profile`'s key: the OS keychain, or on Android files in the
/// app's private storage (`keys_dir`), which no other app reads. Anywhere else without a
/// keychain, `None`: every profile is encrypted, so no key store means the profile does not
/// open.
fn key_store(profile: &Profile, keys_dir: Option<&Path>) -> Option<Box<dyn KeyStore>> {
    if let Some(keychain) = OsKeychain::new() {
        return Some(Box::new(keychain.in_collection(profile.keychain_collection())));
    }
    file_keys(keys_dir).map(|dir| Box::new(beacon_profile::crypto::FileKeyStore::new(dir)) as Box<dyn KeyStore>)
}

/// The profile this process runs. Resolves it on first use for hosts that never call
/// [`init`]; a profile that cannot be opened ends the process, since running without one
/// would write the databases somewhere unintended. The engine and the profile files reach
/// it only after [`lock_profile`] or [`profile_key`], which report that error instead.
pub fn profile() -> &'static Profile {
    match init() {
        Ok(profile) => profile,
        Err(e) => {
            eprintln!("gosub-beacon: cannot open the profile: {e:#}");
            std::process::exit(1);
        }
    }
}

/// Whether this run is the default profile, as opposed to one picked with `--profile` or
/// `--user-data-dir`. A `--profile` naming the default counts as the default.
pub fn is_default() -> bool {
    if crate::cli::Cli::global().user_data_dir.is_some() {
        return false;
    }
    let default = Roots::platform().and_then(|roots| roots.default_id()).ok().flatten();
    default == Some(profile().id())
}

/// The profile folder: settings, cookies, places, localStorage, session.
pub fn profile_dir() -> &'static Path {
    &profile().dir
}

/// The profile's cache folder, outside the profile. Safe to delete at any time.
pub fn cache_dir() -> &'static Path {
    &profile().cache_dir
}

/// Take the profile for this process, once. Every window's engine calls this; only the
/// first takes the lock. Not done in [`init`]: a second GTK launch runs `main` too, and
/// must get as far as handing its URLs to the running instance rather than fail here.
/// Not atomic across threads; every caller today is on the main thread.
pub fn lock_profile() -> anyhow::Result<()> {
    if LOCK.get().is_some() {
        return Ok(());
    }
    // `init`, not `profile`: a host on the C ABI gets here first, and a profile that cannot
    // be opened must come back as an error (`beacon_new` returns NULL), not end its process.
    let lock = init()?.lock()?;
    let _ = LOCK.set(lock);
    Ok(())
}

/// The key each store's database opens with. Every profile is encrypted: without a
/// keychain to hold the key this fails, and Beacon does not start. Resolved once, after
/// [`lock_profile`]: the first call gives a profile without a key one, and encrypts the
/// plaintext databases already in the folder.
pub fn store_key(store: Store) -> anyhow::Result<SqliteKey> {
    Ok(SqliteKey::from_bytes(*profile_key()?.store_key(store)))
}

/// Resolve the profile key now, after [`lock_profile`], so a profile that cannot be
/// encrypted (no keychain) or opened (its key is elsewhere) stops the start with its
/// reason instead of leaving a window without an engine. [`store_key`] would get there on
/// the first engine anyway.
pub fn unlock_profile() -> anyhow::Result<()> {
    profile_key().map(|_| ())
}

fn profile_key() -> anyhow::Result<&'static ProfileKey> {
    let key = match KEY.get() {
        Some(key) => key,
        None => {
            anyhow::ensure!(LOCK.get().is_some(), "the profile key is only read with the profile locked");
            // A copy: unlocking may write `[crypto]` into the manifest, and nothing else this
            // process reads depends on it.
            let mut profile = profile().clone();
            let keys = key_store(&profile, keys_dir());
            let key = profile.unlock(keys.as_deref())?;
            // Kept before encrypting: the copy above is gone after this call, and a retry
            // would unlock the stale copy over again. (The keychain now hands back the key
            // it holds rather than taking a second one, so nothing is stranded either way.)
            KEY.get_or_init(|| key)
        }
    };
    if ENCRYPTED.get().is_none() {
        encrypt_plaintext(profile(), key)?;
        let _ = ENCRYPTED.set(());
        log::info!("profile {:?}: databases encrypted, key in the key store", profile().name());
    }
    Ok(key)
}

/// Encrypt every store still in plaintext: the databases of a profile that just got its
/// key, or the rest of them after a run that stopped halfway. Judged per file by its
/// header, so it is safe to run on every start. The small files beside them (the session,
/// the pickers' history) are sealed the same way.
fn encrypt_plaintext(profile: &Profile, key: &ProfileKey) -> anyhow::Result<()> {
    for store in Store::ALL {
        let path = profile.dir.join(store.file_name());
        if is_plaintext_database(&path)? {
            SqliteKey::from_bytes(*key.store_key(store))
                .encrypt_file(&path)
                .map_err(|e| anyhow::anyhow!("encrypting {}: {e}", path.display()))?;
            log::info!("encrypted {}", path.display());
        }
    }
    profile.seal_plaintext_files(key)
}

/// One of the profile's small files (`beacon_profile::SEALED_FILES`), decrypted; `None`
/// when there is none. Only once the profile is locked and unlocked, like the stores.
pub fn read_profile_file(name: &str) -> anyhow::Result<Option<Vec<u8>>> {
    // The key first: it fails while the profile is not locked, where `profile()` would
    // end the process of a C ABI host.
    let key = profile_key()?;
    profile().read_sealed(key, name)
}

/// Write one of the profile's small files, sealed with the profile key.
pub fn write_profile_file(name: &str, plaintext: &[u8]) -> anyhow::Result<()> {
    let key = profile_key()?;
    profile().write_sealed(key, name, plaintext)
}
