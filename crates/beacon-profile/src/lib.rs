//! Beacon profiles on disk.
//!
//! A profile is one folder that moves between machines as a unit: settings, cookies,
//! bookmarks, history, site storage and the open tabs, plus a `profile.toml` that names it
//! and carries the zone id the cookie and storage rows are keyed by. Its cache lives outside
//! it, in the platform cache directory under the same id, so a copied profile simply starts
//! with an empty cache.
//!
//! ```text
//! <data>/gosub-beacon/profiles.toml           which profile opens by default
//! <data>/gosub-beacon/profiles/<id>/          one folder per profile
//!                                  profile.toml
//!                                  lock
//!                                  settings.db, cookies.db, places.db, ...
//! <cache>/gosub-beacon/<id>/                  disposable, never exported
//! ```
//!
//! The databases are encrypted when the machine has a keychain; see [`crypto`].
//!
//! The engine never sees any of this: Beacon opens the stores in the profile folder and
//! hands the engine the handles and the zone id. This crate has no engine or toolkit
//! dependency, so `beaconctl` can share it.

pub mod crypto;

use anyhow::{anyhow, bail, Context, Result};
use crypto::{Crypto, KdfParams, KeyStore, ProfileKey, Store, CRYPTO_VERSION};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The newest manifest layout this build understands. A profile written by a newer Beacon
/// is refused rather than opened, so an older build cannot quietly damage it. Layout 2 adds
/// `[crypto]`: a Beacon that predates it would open the encrypted databases as empty ones.
pub const SCHEMA: u32 = 2;

/// The layout of a profile with no `[crypto]`, which every Beacon with profiles can open.
const SCHEMA_PLAIN: u32 = 1;
/// The layout of an encrypted profile.
const SCHEMA_CRYPTO: u32 = 2;

/// The zone id every Beacon before profiles used, compiled in. A profile adopted from that
/// layout keeps it, or its cookies and localStorage rows would no longer match.
pub const LEGACY_ZONE: Uuid = Uuid::from_u128(0xf1234567_abcd_4000_8000_000000000001);

/// Bumped when the cache layout changes; a cache with another stamp is thrown away.
const CACHE_VERSION: &str = "1";

/// The file that makes a folder a profile.
pub const MANIFEST: &str = "profile.toml";
const LOCK: &str = "lock";
const DEFAULTS: &str = "profiles.toml";

/// What Beacon wrote into its data directory before profiles existed. Moved into the
/// first profile on migration, and the sign that a folder without a manifest is a profile.
const LEGACY_FILES: &[&str] = &[
    "settings.db",
    "cookies.db",
    "places.db",
    "local-storage.db",
    "local-storage.db-wal",
    "local-storage.db-shm",
    "local-storage",
    "session.tabs",
    "picker-swatches.txt",
    "picker-clock.txt",
];

/// Left behind by the shell-side fetcher Beacon dropped; nothing reads it.
const STALE_FILES: &[&str] = &["fetcher-cookies.db"];

/// `profile.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub id: Uuid,
    pub name: String,
    /// The Beacon version that created the profile.
    pub created_by: String,
    /// The Beacon version that last opened it.
    pub last_opened_by: String,
    pub zones: Vec<ZoneEntry>,
    /// How the databases are encrypted; absent when they are not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub crypto: Option<Crypto>,
}

/// One zone in a profile. Beacon runs one today.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ZoneEntry {
    pub id: Uuid,
    pub name: String,
}

impl Manifest {
    fn new(name: &str, zone: Uuid, version: &str) -> Self {
        Self {
            schema: SCHEMA_PLAIN,
            id: Uuid::new_v4(),
            name: name.to_string(),
            created_by: version.to_string(),
            last_opened_by: version.to_string(),
            zones: vec![ZoneEntry {
                id: zone,
                name: "Default".to_string(),
            }],
            crypto: None,
        }
    }

    fn read(dir: &Path) -> Result<Option<Self>> {
        let path = dir.join(MANIFEST);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let manifest: Self = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(manifest))
    }

    fn write(&self, dir: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)?;
        write_atomic(&dir.join(MANIFEST), &text)
    }
}

/// `profiles.toml` at the data root.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Defaults {
    default: Option<Uuid>,
}

/// Which profile to open.
#[derive(Debug, Clone)]
pub enum Selection {
    /// The default profile, migrating the old flat layout or creating one if there is none.
    Default,
    /// A profile by name or id, created under that name if it does not exist yet.
    Named(String),
    /// Exactly this folder, as `--user-data-dir` asks. A manifest is written if it has none.
    Dir(PathBuf),
}

/// The two roots profiles and their caches live under.
#[derive(Debug, Clone)]
pub struct Roots {
    /// `~/.local/share/gosub-beacon` and the like.
    pub data: PathBuf,
    /// `~/.cache/gosub-beacon` and the like.
    pub cache: PathBuf,
}

impl Roots {
    /// The platform's data and cache directories. An error when the platform has no data
    /// directory: writing a profile into whatever the working directory is would scatter
    /// databases across the disk.
    pub fn platform() -> Result<Self> {
        let data = dirs::data_dir()
            .ok_or_else(|| anyhow!("this platform reports no data directory; pass --user-data-dir"))?
            .join("gosub-beacon");
        let cache = dirs::cache_dir()
            .map(|c| c.join("gosub-beacon"))
            .unwrap_or_else(|| data.join("cache"));
        Ok(Self { data, cache })
    }

    pub fn profiles_dir(&self) -> PathBuf {
        self.data.join("profiles")
    }

    /// Every profile under the profiles folder, by name. Folders without a manifest are
    /// skipped; a manifest that cannot be read is an error, not a missing profile, or Beacon
    /// would quietly start a fresh one next to it.
    pub fn list(&self) -> Result<Vec<Profile>> {
        let mut out = Vec::new();
        let entries = match fs::read_dir(self.profiles_dir()) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e).context("reading the profiles folder"),
        };
        for entry in entries {
            let dir = entry?.path();
            if !dir.is_dir() {
                continue;
            }
            if let Some(manifest) = Manifest::read(&dir)? {
                out.push(Profile {
                    cache_dir: self.cache.join(manifest.id.to_string()),
                    dir,
                    manifest,
                });
            }
        }
        out.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        Ok(out)
    }

    /// The id stored as the default, if any.
    pub fn default_id(&self) -> Result<Option<Uuid>> {
        let path = self.data.join(DEFAULTS);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(toml::from_str::<Defaults>(&text)
                .with_context(|| format!("parsing {}", path.display()))?
                .default),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn set_default(&self, id: Uuid) -> Result<()> {
        make_private_dir(&self.data)?;
        let text = toml::to_string_pretty(&Defaults { default: Some(id) })?;
        write_atomic(&self.data.join(DEFAULTS), &text)
    }

    /// A profile by name (case-insensitive) or id.
    pub fn find(&self, key: &str) -> Result<Option<Profile>> {
        let id = Uuid::parse_str(key).ok();
        Ok(self
            .list()?
            .into_iter()
            .find(|p| Some(p.manifest.id) == id || p.manifest.name.eq_ignore_ascii_case(key)))
    }

    /// A new, empty profile. The first one made becomes the default.
    pub fn create(&self, name: &str, version: &str) -> Result<Profile> {
        let name = name.trim();
        if name.is_empty() {
            bail!("a profile needs a name");
        }
        if self.find(name)?.is_some() {
            bail!("a profile named {name:?} already exists");
        }
        let manifest = Manifest::new(name, Uuid::new_v4(), version);
        let profile = self.place(manifest)?;
        if self.default_id()?.is_none() {
            self.set_default(profile.manifest.id)?;
        }
        Ok(profile)
    }

    /// Make the folder for `manifest` and write it there.
    fn place(&self, manifest: Manifest) -> Result<Profile> {
        make_private_dir(&self.data)?;
        make_private_dir(&self.profiles_dir())?;
        let dir = self.profiles_dir().join(manifest.id.to_string());
        make_private_dir(&dir)?;
        manifest.write(&dir)?;
        Ok(Profile {
            cache_dir: self.cache.join(manifest.id.to_string()),
            dir,
            manifest,
        })
    }

    /// Move the flat layout every Beacon before profiles wrote straight into the data root
    /// into a profile named "default", keeping the old zone id. Does nothing once a
    /// profiles folder exists, or when there is nothing to move.
    pub fn migrate_legacy(&self, version: &str) -> Result<Option<Profile>> {
        if self.profiles_dir().exists() || !holds_legacy_files(&self.data) {
            return Ok(None);
        }
        let profile = self.place(Manifest::new("default", LEGACY_ZONE, version))?;
        for name in LEGACY_FILES {
            let from = self.data.join(name);
            if from.exists() {
                fs::rename(&from, profile.dir.join(name))
                    .with_context(|| format!("moving {} into {}", from.display(), profile.dir.display()))?;
            }
        }
        for name in STALE_FILES {
            let _ = fs::remove_file(self.data.join(name));
        }
        self.set_default(profile.manifest.id)?;
        Ok(Some(profile))
    }

    /// The profile `selection` names, made ready to use: folders private, cache in place,
    /// `last_opened_by` updated. Does not take the lock; see [`Profile::lock`].
    pub fn open(&self, selection: &Selection, version: &str) -> Result<Profile> {
        let mut profile = match selection {
            Selection::Dir(dir) => Profile::adopt(dir, &self.cache, version)?,
            Selection::Named(key) => match self.find(key)? {
                Some(profile) => profile,
                None => self.create(key, version)?,
            },
            Selection::Default => {
                self.migrate_legacy(version)?;
                let by_default = match self.default_id()? {
                    Some(id) => self.find(&id.to_string())?,
                    None => None,
                };
                match by_default {
                    Some(profile) => profile,
                    None => match self.list()?.into_iter().next() {
                        Some(profile) => {
                            self.set_default(profile.manifest.id)?;
                            profile
                        }
                        None => self.create("default", version)?,
                    },
                }
            }
        };
        profile.prepare(version)?;
        Ok(profile)
    }
}

/// An open profile: its folder, its cache folder and its manifest.
#[derive(Debug, Clone)]
pub struct Profile {
    pub dir: PathBuf,
    pub cache_dir: PathBuf,
    pub manifest: Manifest,
}

impl Profile {
    pub fn id(&self) -> Uuid {
        self.manifest.id
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    /// The zone Beacon runs the profile's tabs in.
    pub fn zone_id(&self) -> Uuid {
        self.manifest.zones.first().map_or(LEGACY_ZONE, |z| z.id)
    }

    /// `dir` as a profile, writing a manifest when it has none. A folder already holding
    /// the old flat layout keeps the old zone id, so its cookies and storage still match.
    /// The profile already in `dir`, as it is: nothing is created or written, so it is safe
    /// to read one Beacon has open.
    pub fn at(dir: &Path, cache_root: &Path) -> Result<Self> {
        let Some(manifest) = Manifest::read(dir)? else {
            bail!("{} holds no profile", dir.display());
        };
        Ok(Self {
            cache_dir: cache_root.join(manifest.id.to_string()),
            dir: dir.to_path_buf(),
            manifest,
        })
    }

    fn adopt(dir: &Path, cache_root: &Path, version: &str) -> Result<Self> {
        make_private_dir(dir)?;
        let manifest = match Manifest::read(dir)? {
            Some(manifest) => manifest,
            None => {
                let zone = if holds_legacy_files(dir) { LEGACY_ZONE } else { Uuid::new_v4() };
                let name = dir.file_name().map_or("profile".into(), |n| n.to_string_lossy().into_owned());
                let manifest = Manifest::new(&name, zone, version);
                manifest.write(dir)?;
                manifest
            }
        };
        Ok(Self {
            cache_dir: cache_root.join(manifest.id.to_string()),
            dir: dir.to_path_buf(),
            manifest,
        })
    }

    /// Refuse a newer profile, make the folders private, set up the cache and record this
    /// version as the last to open it.
    fn prepare(&mut self, version: &str) -> Result<()> {
        if self.manifest.schema > SCHEMA {
            bail!(
                "profile {:?} was written by a newer Beacon ({}, profile layout {}); this build understands layout {SCHEMA}",
                self.manifest.name,
                self.manifest.last_opened_by,
                self.manifest.schema
            );
        }
        make_private_dir(&self.dir)?;
        self.prepare_cache()?;
        if self.manifest.last_opened_by != version {
            self.manifest.last_opened_by = version.to_string();
            self.manifest.write(&self.dir)?;
        }
        Ok(())
    }

    /// Create the cache folder, or empty it when its stamp is from another layout. Tagged
    /// as a cache so backup tools that honour `CACHEDIR.TAG` skip it.
    fn prepare_cache(&self) -> Result<()> {
        let stamp = self.cache_dir.join("VERSION");
        if self.cache_dir.exists() && fs::read_to_string(&stamp).ok().as_deref() != Some(CACHE_VERSION) {
            fs::remove_dir_all(&self.cache_dir).with_context(|| format!("clearing {}", self.cache_dir.display()))?;
        }
        if let Some(root) = self.cache_dir.parent() {
            make_private_dir(root)?;
        }
        make_private_dir(&self.cache_dir)?;
        if !stamp.exists() {
            fs::write(&stamp, CACHE_VERSION)?;
            fs::write(
                self.cache_dir.join("CACHEDIR.TAG"),
                "Signature: 8a477f597d28d172789f06886806bc55\n# Gosub Beacon cache; safe to delete.\n",
            )?;
        }
        Ok(())
    }

    /// Take the profile for this process. Held until the returned value is dropped; a
    /// second process gets an error instead of two writers on the same databases.
    pub fn lock(&self) -> Result<ProfileLock> {
        let path = self.dir.join(LOCK);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        match file.try_lock() {
            Ok(()) => Ok(ProfileLock { _file: file }),
            Err(fs::TryLockError::WouldBlock) => {
                bail!(
                    "profile {:?} is in use by another Beacon ({})",
                    self.manifest.name,
                    self.dir.display()
                )
            }
            Err(fs::TryLockError::Error(e)) => Err(e).with_context(|| format!("locking {}", path.display())),
        }
    }

    /// Whether another process holds the lock right now.
    pub fn in_use(&self) -> bool {
        self.lock().is_err()
    }
}

/// Unlocking: what the databases are encrypted with.
impl Profile {
    /// The key this profile's databases are encrypted with. Call with the profile locked.
    ///
    /// A profile without `[crypto]` gets a key here, and the caller encrypts the plaintext
    /// databases already in it. Every profile is encrypted: with no keychain, or one that
    /// does not keep the key, this is an error and the profile does not open.
    ///
    /// The key is a new one only when the keychain holds none for this profile. One it
    /// does hold is adopted as it is: the manifest lost its `[crypto]` (restored from a
    /// backup, a sync conflict, a crash between filing the key and writing the manifest),
    /// and a fresh key filed over it would leave the databases it encrypted unopenable
    /// for good. Databases that are already encrypted with no key anywhere are refused for
    /// the same reason, rather than written off as something to encrypt around.
    ///
    /// An encrypted profile needs its key from `keys`. When the keychain does not hold it
    /// (a profile copied from another machine), the error is a [`Locked`]: unlock it with
    /// the passphrase first ([`Profile::unlock_with_passphrase`]).
    pub fn unlock(&mut self, keys: Option<&dyn KeyStore>) -> Result<ProfileKey> {
        if self.manifest.crypto.is_some() {
            return self.key_from(keys);
        }
        let Some(keys) = keys else {
            bail!(
                "profile {:?} cannot be encrypted: this platform has no keychain Beacon can use",
                self.name()
            );
        };
        let key = match keys.get(self.id())? {
            Some(key) => key,
            None if self.has_encrypted_store() => bail!(
                "profile {:?} has encrypted databases, but its manifest does not say so and this machine's keychain has no key for it; \
                 restore {MANIFEST} from the machine that encrypted it",
                self.name()
            ),
            None => {
                let key = ProfileKey::generate(self.id());
                let kept = file_key(keys, self.id(), &key)
                    .with_context(|| format!("profile {:?} cannot be encrypted: no keychain to keep its key", self.name()))?;
                if !kept {
                    bail!("profile {:?} cannot be encrypted: the keychain did not keep its key", self.name());
                }
                key
            }
        };
        self.manifest.crypto = Some(key.manifest_entry());
        self.manifest.schema = self.manifest.schema.max(SCHEMA_CRYPTO);
        self.manifest.write(&self.dir)?;
        Ok(key)
    }

    /// The key of an encrypted profile, from the keychain, checked against the manifest.
    pub fn key_from(&self, keys: Option<&dyn KeyStore>) -> Result<ProfileKey> {
        let crypto = self.crypto()?;
        let found = match keys {
            Some(keys) => keys.get(self.id())?,
            None => None,
        };
        match found {
            Some(key) if key.check() == crypto.check => Ok(key),
            Some(_) => Err(anyhow!(Locked {
                id: self.id(),
                name: self.name().to_string(),
                passphrase: crypto.passphrase.is_some(),
                stale: true,
            })),
            None => Err(anyhow!(Locked {
                id: self.id(),
                name: self.name().to_string(),
                passphrase: crypto.passphrase.is_some(),
                stale: false,
            })),
        }
    }

    /// Unwrap the profile key with `passphrase` and file it in `keys`, so later runs open
    /// the profile without asking. Call with the profile locked.
    pub fn unlock_with_passphrase(&self, keys: &dyn KeyStore, passphrase: &str) -> Result<ProfileKey> {
        let key = self.key_from_passphrase(passphrase)?;
        if !file_key(keys, self.id(), &key)? {
            bail!("the keychain did not keep the key for profile {:?}", self.name());
        }
        Ok(key)
    }

    /// The profile key, unwrapped with `passphrase`: also the proof that the passphrase
    /// still opens the profile, before this machine's key is given up.
    pub fn key_from_passphrase(&self, passphrase: &str) -> Result<ProfileKey> {
        let crypto = self.crypto()?;
        let Some(wrapped) = &crypto.passphrase else {
            bail!("profile {:?} has no passphrase to unlock it with", self.name());
        };
        let key = ProfileKey::unwrap(self.id(), wrapped, passphrase)?;
        if key.check() != crypto.check {
            bail!("profile {:?}: the passphrase opens a key that is not this profile's", self.name());
        }
        Ok(key)
    }

    /// Wrap `key` with `passphrase` into the manifest, replacing any passphrase it had.
    /// The profile can then be opened on another machine. Call with the profile locked.
    pub fn set_passphrase(&mut self, key: &ProfileKey, passphrase: &str, params: KdfParams) -> Result<()> {
        if passphrase.is_empty() {
            bail!("an empty passphrase protects nothing");
        }
        let crypto = self.crypto()?;
        if key.check() != crypto.check {
            bail!("that key is not profile {:?}'s", self.name());
        }
        let wrapped = key.wrap(passphrase, params)?;
        if let Some(crypto) = &mut self.manifest.crypto {
            crypto.passphrase = Some(wrapped);
        }
        self.manifest.write(&self.dir)
    }

    /// Drop the passphrase: the profile opens only where the keychain holds its key.
    pub fn remove_passphrase(&mut self) -> Result<()> {
        self.crypto()?;
        if let Some(crypto) = &mut self.manifest.crypto {
            crypto.passphrase = None;
        }
        self.manifest.write(&self.dir)
    }

    /// Whether any store in the folder is a database that is not plaintext SQLite. Judged by
    /// the file header alone, as the engine judges what to encrypt: this crate does not
    /// open databases.
    fn has_encrypted_store(&self) -> bool {
        Store::ALL
            .iter()
            .any(|store| is_encrypted_database(&self.dir.join(store.file_name())))
    }

    fn crypto(&self) -> Result<&Crypto> {
        let Some(crypto) = &self.manifest.crypto else {
            bail!("profile {:?} is not encrypted", self.name());
        };
        if crypto.version > CRYPTO_VERSION {
            bail!(
                "profile {:?} is encrypted with a newer scheme ({}) than this Beacon knows ({CRYPTO_VERSION})",
                self.name(),
                crypto.version
            );
        }
        Ok(crypto)
    }
}

/// An encrypted profile whose key this machine's keychain does not have (or has a stale
/// one of). Find it with `err.downcast_ref::<Locked>()`.
#[derive(Debug)]
pub struct Locked {
    pub id: Uuid,
    pub name: String,
    /// The profile carries a passphrase-wrapped key, so the passphrase can unlock it.
    pub passphrase: bool,
    /// The keychain holds a key under this profile's id, but not this profile's.
    pub stale: bool,
}

impl std::fmt::Display for Locked {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let held = if self.stale {
            "the keychain holds a different key for it"
        } else {
            "its key is not in this machine's keychain"
        };
        if self.passphrase {
            // The name when it can be pasted into a shell as it is, the id otherwise.
            let plain = !self.name.starts_with('-') && self.name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
            let arg = if plain { self.name.clone() } else { self.id.to_string() };
            write!(
                f,
                "profile {:?} is encrypted and {held}; unlock it with `beaconctl profile unlock {arg}`",
                self.name
            )
        } else {
            write!(
                f,
                "profile {:?} is encrypted, {held}, and it has no passphrase: it can only be opened on the machine that made it",
                self.name
            )
        }
    }
}

impl std::error::Error for Locked {}

/// File `key` in `keys` and read it back: a keychain that accepts a key and loses it would
/// leave databases nothing can open. `Ok(false)` when it did not keep it.
fn file_key(keys: &dyn KeyStore, profile: Uuid, key: &ProfileKey) -> Result<bool> {
    keys.set(key)?;
    Ok(keys.get(profile)?.is_some_and(|back| back.check() == key.check()))
}

/// Held for as long as a process uses a profile.
#[derive(Debug)]
pub struct ProfileLock {
    _file: File,
}

fn holds_legacy_files(dir: &Path) -> bool {
    LEGACY_FILES.iter().any(|name| dir.join(name).exists())
}

/// Whether `path` holds a database that is not plaintext SQLite: a full header that is not
/// SQLite's. A missing, empty or short file is nothing at all, and an unreadable one is
/// left for whatever opens it to report.
fn is_encrypted_database(path: &Path) -> bool {
    use std::io::Read;
    let mut header = [0u8; 16];
    match File::open(path).and_then(|mut f| f.read_exact(&mut header)) {
        Ok(()) => &header != b"SQLite format 3\0",
        Err(_) => false,
    }
}

/// Create `dir` and make it readable by the owner only: a profile keeps its session file
/// and picker history in plain text, and its databases too until the first start encrypts
/// them; the folder mode is what keeps other users out of every file inside.
fn make_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).with_context(|| format!("setting the mode of {}", dir.display()))?;
    }
    Ok(())
}

/// Write via a temp file and rename, so a crash leaves the old file or the new one.
fn write_atomic(path: &Path, text: &str) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: &str = "0.1.0";

    fn roots(tmp: &tempfile::TempDir) -> Roots {
        Roots {
            data: tmp.path().join("data"),
            cache: tmp.path().join("cache"),
        }
    }

    #[test]
    fn empty_root_gets_a_default_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(p.name(), "default");
        assert_ne!(p.zone_id(), LEGACY_ZONE);
        assert_eq!(roots.default_id().unwrap(), Some(p.id()));
        assert_eq!(p.dir, roots.profiles_dir().join(p.id().to_string()));
        assert!(p.cache_dir.join("CACHEDIR.TAG").exists());
        // Opened again: the same profile, not a second one.
        assert_eq!(roots.open(&Selection::Default, V).unwrap().id(), p.id());
        assert_eq!(roots.list().unwrap().len(), 1);
    }

    #[test]
    fn flat_layout_moves_into_a_profile_with_the_old_zone() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        fs::create_dir_all(roots.data.join("local-storage")).unwrap();
        fs::write(roots.data.join("cookies.db"), "c").unwrap();
        fs::write(roots.data.join("session.tabs"), "s").unwrap();
        fs::write(roots.data.join("fetcher-cookies.db"), "x").unwrap();
        let p = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(p.zone_id(), LEGACY_ZONE);
        assert_eq!(fs::read_to_string(p.dir.join("cookies.db")).unwrap(), "c");
        assert!(p.dir.join("session.tabs").exists());
        assert!(p.dir.join("local-storage").is_dir());
        assert!(!roots.data.join("cookies.db").exists());
        assert!(!roots.data.join("fetcher-cookies.db").exists());
    }

    #[test]
    fn named_profile_is_created_then_found() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let home = roots.open(&Selection::Default, V).unwrap();
        let work = roots.open(&Selection::Named("Work".into()), V).unwrap();
        assert_ne!(work.id(), home.id());
        assert_eq!(roots.open(&Selection::Named("work".into()), V).unwrap().id(), work.id());
        assert_eq!(roots.open(&Selection::Named(work.id().to_string()), V).unwrap().id(), work.id());
        // The default did not move to the new profile.
        assert_eq!(roots.default_id().unwrap(), Some(home.id()));
        assert!(roots.create("WORK", V).is_err());
    }

    #[test]
    fn exact_dir_is_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let fresh = tmp.path().join("scratch");
        let p = roots.open(&Selection::Dir(fresh.clone()), V).unwrap();
        assert_eq!(p.dir, fresh);
        assert_ne!(p.zone_id(), LEGACY_ZONE);
        assert!(fresh.join(MANIFEST).exists());

        let old = tmp.path().join("old");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("places.db"), "p").unwrap();
        let p = roots.open(&Selection::Dir(old.clone()), V).unwrap();
        assert_eq!(p.zone_id(), LEGACY_ZONE);
        assert_eq!(roots.open(&Selection::Dir(old), V).unwrap().id(), p.id());
    }

    #[test]
    fn manifest_round_trips_and_records_the_version() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, "0.1.0").unwrap();
        let again = roots.open(&Selection::Default, "0.2.0").unwrap();
        assert_eq!(again.manifest.created_by, "0.1.0");
        assert_eq!(again.manifest.last_opened_by, "0.2.0");
        assert_eq!(Manifest::read(&p.dir).unwrap().unwrap(), again.manifest);
    }

    #[test]
    fn newer_profile_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let mut p = roots.open(&Selection::Default, V).unwrap();
        p.manifest.schema = SCHEMA + 1;
        p.manifest.write(&p.dir).unwrap();
        let err = roots.open(&Selection::Default, V).unwrap_err();
        assert!(err.to_string().contains("newer Beacon"), "{err}");
    }

    #[test]
    fn broken_manifest_stops_rather_than_starting_over() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        fs::write(p.dir.join(MANIFEST), "not = [toml").unwrap();
        assert!(roots.open(&Selection::Default, V).is_err());
        assert!(roots.open(&Selection::Named("work".into()), V).is_err());
        assert_eq!(fs::read_dir(roots.profiles_dir()).unwrap().count(), 1);
    }

    #[test]
    fn second_lock_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        let held = p.lock().unwrap();
        assert!(p.lock().is_err());
        drop(held);
        assert!(p.lock().is_ok());
    }

    #[test]
    fn stale_cache_is_cleared() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        fs::write(p.cache_dir.join("VERSION"), "0").unwrap();
        fs::write(p.cache_dir.join("old-entry"), "x").unwrap();
        roots.open(&Selection::Default, V).unwrap();
        assert!(!p.cache_dir.join("old-entry").exists());
        assert_eq!(fs::read_to_string(p.cache_dir.join("VERSION")).unwrap(), CACHE_VERSION);
    }

    #[test]
    fn a_profile_gets_a_key_once_and_keeps_it() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&keys)).unwrap();
        let written = Manifest::read(&p.dir).unwrap().unwrap();
        assert_eq!(written.schema, SCHEMA_CRYPTO);
        assert!(written.crypto.is_some());

        let mut again = roots.open(&Selection::Default, V).unwrap();
        let same = again.unlock(Some(&keys)).unwrap();
        assert_eq!(same.check(), key.check());
    }

    /// `profile.toml` as it was before the profile got its key: what a backup or a sync
    /// conflict puts back.
    fn roll_back_manifest(p: &Profile) {
        let mut old = p.manifest.clone();
        old.crypto = None;
        old.schema = SCHEMA_PLAIN;
        old.write(&p.dir).unwrap();
    }

    #[test]
    fn a_manifest_that_lost_its_crypto_takes_the_keychain_key_back() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&keys)).unwrap();
        // The databases are encrypted with that key by now.
        fs::write(p.dir.join(Store::Cookies.file_name()), b"not SQLite at all, sixteen bytes and more").unwrap();
        roll_back_manifest(&p);

        let mut again = roots.open(&Selection::Default, V).unwrap();
        assert!(again.manifest.crypto.is_none());
        let same = again.unlock(Some(&keys)).unwrap();
        assert_eq!(same.check(), key.check());
        let written = Manifest::read(&p.dir).unwrap().unwrap();
        assert_eq!(written.crypto.map(|c| c.check), Some(key.check()));
        assert_eq!(written.schema, SCHEMA_CRYPTO);
        // The keychain still holds the one key that opens the databases.
        assert_eq!(keys.get(p.id()).unwrap().unwrap().check(), key.check());
    }

    #[test]
    fn encrypted_databases_without_a_key_anywhere_are_not_rekeyed() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let mut p = roots.open(&Selection::Default, V).unwrap();
        p.unlock(Some(&crypto::MemoryKeyStore::default())).unwrap();
        fs::write(p.dir.join(Store::Places.file_name()), b"not SQLite at all, sixteen bytes and more").unwrap();
        roll_back_manifest(&p);

        // Another machine: its keychain knows nothing of this profile.
        let there = crypto::MemoryKeyStore::default();
        let mut copy = roots.open(&Selection::Default, V).unwrap();
        let err = copy.unlock(Some(&there)).unwrap_err();
        assert!(err.to_string().contains("encrypted databases"), "{err}");
        assert!(Manifest::read(&p.dir).unwrap().unwrap().crypto.is_none());
        assert!(there.get(p.id()).unwrap().is_none());

        // Plaintext databases, or none, are what a new key is for.
        fs::write(p.dir.join(Store::Places.file_name()), b"SQLite format 3\0 and a page of plaintext").unwrap();
        fs::write(p.dir.join(Store::Cookies.file_name()), b"").unwrap();
        let mut copy = roots.open(&Selection::Default, V).unwrap();
        copy.unlock(Some(&there)).unwrap();
        assert!(there.get(p.id()).unwrap().is_some());
    }

    #[test]
    fn without_a_keychain_the_profile_does_not_open() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let mut p = roots.open(&Selection::Default, V).unwrap();
        assert!(p.unlock(None).is_err());
        let refusing = crypto::MemoryKeyStore::refusing();
        assert!(p.unlock(Some(&refusing)).is_err());
        let written = Manifest::read(&p.dir).unwrap().unwrap();
        assert_eq!((written.schema, written.crypto), (SCHEMA_PLAIN, None));
    }

    #[test]
    fn a_copied_profile_needs_its_passphrase_once() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let here = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&here)).unwrap();

        // Another machine, no passphrase yet: nothing opens it.
        let there = crypto::MemoryKeyStore::default();
        let mut copy = roots.open(&Selection::Default, V).unwrap();
        let err = copy.unlock(Some(&there)).unwrap_err();
        let locked = err.downcast_ref::<Locked>().expect("a Locked error");
        assert!(!locked.passphrase && !locked.stale, "{locked}");

        p.set_passphrase(&key, "hunter2", crypto::TEST_KDF).unwrap();
        let mut copy = roots.open(&Selection::Default, V).unwrap();
        assert!(copy.unlock(Some(&there)).unwrap_err().downcast_ref::<Locked>().unwrap().passphrase);
        assert!(copy.unlock_with_passphrase(&there, "wrong").is_err());
        copy.unlock_with_passphrase(&there, "hunter2").unwrap();
        assert_eq!(copy.unlock(Some(&there)).unwrap().check(), key.check());
    }

    #[test]
    fn a_passphrase_unlock_into_a_keychain_that_loses_the_key_fails() {
        struct Forgetful;
        impl KeyStore for Forgetful {
            fn get(&self, _profile: Uuid) -> Result<Option<ProfileKey>> {
                Ok(None)
            }
            fn set(&self, _key: &ProfileKey) -> Result<()> {
                Ok(())
            }
            fn delete(&self, _profile: Uuid) -> Result<()> {
                Ok(())
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&crypto::MemoryKeyStore::default())).unwrap();
        p.set_passphrase(&key, "pw", crypto::TEST_KDF).unwrap();
        assert_eq!(p.key_from_passphrase("pw").unwrap().check(), key.check());
        assert!(p.key_from_passphrase("wrong").is_err());
        let err = p.unlock_with_passphrase(&Forgetful, "pw").unwrap_err();
        assert!(err.to_string().contains("did not keep"), "{err}");
    }

    #[test]
    fn the_unlock_hint_uses_the_id_for_a_name_a_shell_would_split() {
        let id = Uuid::new_v4();
        let locked = |name: &str| Locked {
            id,
            name: name.to_string(),
            passphrase: true,
            stale: false,
        };
        assert!(locked("work").to_string().ends_with("`beaconctl profile unlock work`"));
        assert!(locked("my work").to_string().ends_with(&format!("`beaconctl profile unlock {id}`")));
    }

    #[test]
    fn a_stale_keychain_key_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        p.unlock(Some(&keys)).unwrap();
        keys.set(&ProfileKey::generate(p.id())).unwrap();
        let err = p.unlock(Some(&keys)).unwrap_err();
        assert!(err.downcast_ref::<Locked>().unwrap().stale);
    }

    #[test]
    fn removing_the_passphrase_keeps_the_keychain_key() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&keys)).unwrap();
        assert!(p.set_passphrase(&key, "", crypto::TEST_KDF).is_err());
        p.set_passphrase(&key, "pw", crypto::TEST_KDF).unwrap();
        p.remove_passphrase().unwrap();
        let written = Manifest::read(&p.dir).unwrap().unwrap();
        assert!(written.crypto.unwrap().passphrase.is_none());
        p.unlock(Some(&keys)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn folders_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        for dir in [&roots.data, &p.dir, &p.cache_dir] {
            assert_eq!(fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700, "{}", dir.display());
        }
    }
}
