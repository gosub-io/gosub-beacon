//! Where Beacon keeps a profile on disk.
//!
//! The layout itself (folders, `profile.toml`, the lock, migrating the old flat layout) is
//! the `beacon-profile` crate's. This module picks the profile from the command line, once
//! per process, and hands out its folders.

use beacon_profile::crypto::{OsKeychain, ProfileKey, Store};
use beacon_profile::{Profile, ProfileLock, Roots, Selection};
use gosub_engine::storage::{is_plaintext_database, SqliteKey};
use std::path::Path;
use std::sync::OnceLock;

static PROFILE: OnceLock<Profile> = OnceLock::new();
static LOCK: OnceLock<ProfileLock> = OnceLock::new();
static KEY: OnceLock<Option<ProfileKey>> = OnceLock::new();

/// Resolve the profile from `--user-data-dir` / `--profile`, creating or migrating it as
/// needed. Call early in `main`, after the command line and before the lockdown, which
/// only lets this process write inside the profile and its cache. Nothing is logged here:
/// the logger does not exist yet, so the error goes back to the caller to print.
pub fn init() -> anyhow::Result<&'static Profile> {
    if let Some(profile) = PROFILE.get() {
        return Ok(profile);
    }
    let cli = crate::cli::Cli::global();
    let selection = match (&cli.user_data_dir, &cli.profile) {
        (Some(dir), _) => Selection::Dir(dir.clone()),
        (None, Some(name)) => Selection::Named(name.clone()),
        (None, None) => Selection::Default,
    };
    let roots = match &selection {
        // An exact folder still needs a cache root; without a platform one (Android has
        // none through `dirs`) the cache goes next to the folder, never inside it.
        Selection::Dir(dir) => Roots::platform().unwrap_or_else(|_| Roots {
            data: dir.clone(),
            cache: dir.parent().unwrap_or(dir).join("cache"),
        }),
        _ => Roots::platform()?,
    };
    let profile = roots.open(&selection, env!("CARGO_PKG_VERSION"))?;
    Ok(PROFILE.get_or_init(|| profile))
}

/// The profile this process runs. Resolves it on first use for hosts that never call
/// [`init`] (the C ABI); a profile that cannot be opened ends the process, since running
/// without one would write the databases somewhere unintended.
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
    let lock = profile().lock()?;
    let _ = LOCK.set(lock);
    Ok(())
}

/// The key each store's database opens with, `None` when the profile is not encrypted.
/// Resolved once, after [`lock_profile`]: the first call gives a profile without a key one
/// when the keychain takes it, and encrypts the plaintext databases already in the folder.
pub fn store_key(store: Store) -> anyhow::Result<Option<SqliteKey>> {
    Ok(profile_key()?.map(|key| SqliteKey::from_bytes(*key.store_key(store))))
}

fn profile_key() -> anyhow::Result<Option<&'static ProfileKey>> {
    if let Some(key) = KEY.get() {
        return Ok(key.as_ref());
    }
    anyhow::ensure!(LOCK.get().is_some(), "the profile key is only read with the profile locked");
    // A copy: unlocking may write `[crypto]` into the manifest, and nothing else this
    // process reads depends on it.
    let mut profile = profile().clone();
    let keychain = OsKeychain::new();
    let key = profile.unlock(keychain.as_ref().map(|k| k as &dyn beacon_profile::crypto::KeyStore))?;
    if let Some(key) = &key {
        encrypt_plaintext(&profile, key)?;
        log::info!("profile {:?}: databases encrypted, key in the keychain", profile.name());
    }
    Ok(KEY.get_or_init(|| key).as_ref())
}

/// Encrypt every store still in plaintext: the databases of a profile that just got its
/// key, or the rest of them after a run that stopped halfway. Judged per file by its
/// header, so it is safe to run on every start.
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
    Ok(())
}
