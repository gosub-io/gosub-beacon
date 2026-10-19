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
use crypto::{Crypto, KdfParams, KeyStore, ProfileKey, Slot, Store, CRYPTO_VERSION};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use zeroize::Zeroizing;

/// The newest manifest layout this build understands. A profile written by a newer Beacon
/// is refused rather than opened, so an older build cannot quietly damage it. Layout 2 adds
/// `[crypto]`: a Beacon that predates it would open the encrypted databases as empty ones.
/// Layout 3 adds `keychain` and `keychain_leftover`: a Beacon that predates it would drop
/// them on its next manifest write and look for the key in the default collection only.
pub const SCHEMA: u32 = 3;

/// The layout of a profile with no `[crypto]`, which every Beacon with profiles can open.
const SCHEMA_PLAIN: u32 = 1;
/// The layout of an encrypted profile.
const SCHEMA_CRYPTO: u32 = 2;
/// The layout of a profile whose key is, or was, in a keychain collection of its own.
const SCHEMA_KEYCHAIN: u32 = 3;

/// The zone id every Beacon before profiles used, compiled in. A profile adopted from that
/// layout keeps it, or its cookies and localStorage rows would no longer match.
pub const LEGACY_ZONE: Uuid = Uuid::from_u128(0xf1234567_abcd_4000_8000_000000000001);

/// Bumped when the cache layout changes; a cache with another stamp is thrown away.
const CACHE_VERSION: &str = "1";

/// The file that makes a folder a profile.
pub const MANIFEST: &str = "profile.toml";
/// How the manifest names the keychain's default collection where it has to name one. It
/// is the Secret Service's own name for it too, so it is never a collection of its own.
pub const DEFAULT_COLLECTION: &str = "default";
const LOCK: &str = "lock";
const DEFAULTS: &str = "profiles.toml";
/// At the data root: held while a profile is picked, created or migrated.
const ROOTS_LOCK: &str = "profiles.lock";

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

/// The small files beside the databases, each sealed with the profile key
/// ([`ProfileKey::seal`]): the open tabs and the pickers' history. Read and written through
/// [`Profile::read_sealed`] and [`Profile::write_sealed`].
pub const SEALED_FILES: &[&str] = &["session.tabs", "picker-swatches.txt", "picker-clock.txt"];

/// `profile.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    /// Never changes for a folder: it salts every key derived from the profile key, binds
    /// the passphrase-wrapped key, and names the keychain entry. A folder given a new id
    /// would open nothing.
    pub id: Uuid,
    pub name: String,
    /// The Beacon version that created the profile.
    pub created_by: String,
    /// The Beacon version that last opened it.
    pub last_opened_by: String,
    /// The keychain collection holding the profile key, on Linux the label of a Secret
    /// Service collection; absent for the default one (the login keyring, unlocked when
    /// the user logs in). A collection of its own, with its own password, asks for that
    /// password before Beacon gets the key. See [`crypto::OsKeychain::in_collection`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain: Option<String>,
    /// The collection a [`Profile::move_key`] left a copy of the key in, until that copy is
    /// removed: [`DEFAULT_COLLECTION`] for the default one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keychain_leftover: Option<String>,
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
            keychain: None,
            keychain_leftover: None,
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

    /// Refuse a manifest from a newer Beacon: this build would write it back with only
    /// the fields it knows, dropping the rest.
    fn ensure_understood(&self) -> Result<()> {
        if self.schema > SCHEMA {
            bail!(
                "profile {:?} was written by a newer Beacon ({}, profile layout {}); this build understands layout {SCHEMA}",
                self.name,
                self.last_opened_by,
                self.schema
            );
        }
        Ok(())
    }

    fn write(&self, dir: &Path) -> Result<()> {
        self.ensure_understood()?;
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
            match Manifest::read(&dir)? {
                Some(manifest) => out.push(Profile {
                    cache_dir: self.cache.join(manifest.id.to_string()),
                    dir,
                    manifest,
                }),
                None if holds_encrypted_stores(&dir) || holds_legacy_files(&dir) => log::warn!(
                    "{} holds a profile's files but no {MANIFEST}, so it is skipped; restore the manifest from a backup to get the profile back",
                    dir.display()
                ),
                None => {}
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

    /// A new, empty profile. The first one made becomes the default. The old flat layout is
    /// migrated first, so it still becomes the profile it was.
    pub fn create(&self, name: &str, version: &str) -> Result<Profile> {
        let _lock = self.lock()?;
        self.migrate_legacy(version)?;
        self.create_locked(name, version)
    }

    /// Hold the data root while a profile is picked, created or migrated. Two Beacons
    /// started at once would otherwise each make a "default" and run apart. Blocks: the
    /// other side holds it for a moment only.
    fn lock(&self) -> Result<File> {
        make_private_dir(&self.data)?;
        let path = self.data.join(ROOTS_LOCK);
        let file = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.lock().with_context(|| format!("locking {}", path.display()))?;
        Ok(file)
    }

    fn create_locked(&self, name: &str, version: &str) -> Result<Profile> {
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
    /// into a profile named "default", keeping the old zone id, and make it the default.
    /// Judged by the old files alone: a profiles folder can exist before Beacon first starts
    /// (`beaconctl profile create`). A move that stopped partway is finished into the
    /// profile it started, the one on the old zone id. Call with the roots locked.
    fn migrate_legacy(&self, version: &str) -> Result<Option<Profile>> {
        if !holds_legacy_files(&self.data) {
            return Ok(None);
        }
        let started = self.list()?.into_iter().find(|p| p.zone_id() == LEGACY_ZONE);
        let fresh = started.is_none();
        let profile = match started {
            Some(profile) => profile,
            None => {
                let name = if self.find("default")?.is_some() {
                    "default (migrated)"
                } else {
                    "default"
                };
                self.place(Manifest::new(name, LEGACY_ZONE, version))?
            }
        };
        for name in LEGACY_FILES {
            let from = self.data.join(name);
            let to = profile.dir.join(name);
            if !from.exists() {
                continue;
            }
            // The profile's own copy is the newer one; the old file stays where it is.
            if to.exists() {
                log::warn!("{} is left where it is: {} has one already", from.display(), profile.dir.display());
                continue;
            }
            fs::rename(&from, &to).with_context(|| format!("moving {} into {}", from.display(), profile.dir.display()))?;
        }
        for name in STALE_FILES {
            let _ = fs::remove_file(self.data.join(name));
        }
        // A move that stopped before the default was written still makes it, or the next
        // profile created would take the default over the old data.
        if fresh || self.default_id()?.is_none() {
            self.set_default(profile.manifest.id)?;
        }
        Ok(Some(profile))
    }

    /// The profile `selection` names, made ready to use: folders private, cache in place,
    /// `last_opened_by` updated, unless another process holds it. Does not take the lock;
    /// see [`Profile::lock`].
    pub fn open(&self, selection: &Selection, version: &str) -> Result<Profile> {
        let _lock = match selection {
            Selection::Dir(_) => None,
            _ => Some(self.lock()?),
        };
        let mut profile = match selection {
            Selection::Dir(dir) => Profile::adopt(dir, &self.cache, version)?,
            Selection::Named(key) => {
                self.migrate_legacy(version)?;
                match self.find(key)? {
                    Some(profile) => profile,
                    None => self.create_locked(key, version)?,
                }
            }
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
                        None => self.create_locked("default", version)?,
                    },
                }
            }
        };
        // Another process has it open: its cache and manifest are that process's. This one
        // only gets as far as handing over to it, or being refused the lock.
        if profile.in_use() {
            profile.manifest.ensure_understood()?;
        } else {
            // A folder named on the command line is the user's: its mode is set only when
            // this made it.
            profile.prepare(version, !matches!(selection, Selection::Dir(_)))?;
        }
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

    /// The keychain collection the profile key is kept in; `None` for the default one.
    pub fn keychain_collection(&self) -> Option<&str> {
        self.manifest.keychain.as_deref()
    }

    /// Move the profile key from the collection in `from` to the one in `to`, and record
    /// `to`'s collection in the manifest. Call with the profile locked. The key is filed
    /// in `to` and read back before the manifest names it and `from` lets go of it, so a
    /// move that stops partway leaves the key in both, never in neither. The manifest
    /// names the copy left in `from` until it is gone; [`Profile::remove_leftover_key`]
    /// finishes a move that stopped there. A profile without a key yet only records the
    /// collection, and gets its key there on first open.
    pub fn move_key(&mut self, from: &dyn KeyStore, to: &dyn KeyStore, collection: Option<&str>) -> Result<()> {
        if self.manifest.crypto.is_none() {
            self.manifest.keychain = collection.map(str::to_string);
            self.manifest.schema = self.manifest.schema.max(SCHEMA_KEYCHAIN);
            return self.manifest.write(&self.dir);
        }
        // The manifest names one leftover copy; a second move would lose the first.
        if let Some(leftover) = self.leftover_key_collection() {
            bail!(
                "profile {:?} still has a copy of its key in the keychain collection {leftover:?}; remove that first",
                self.name()
            );
        }
        self.ensure_not_rotating(from)?;
        let key = self.key_from(Some(from))?;
        to.set(&key, Slot::Current)?;
        // Checked against the manifest like any key from a keychain.
        self.key_from(Some(to))
            .context("the key did not read back from the new keychain collection")?;
        let old = self.keychain_collection().unwrap_or(DEFAULT_COLLECTION).to_string();
        self.manifest.keychain = collection.map(str::to_string);
        self.manifest.keychain_leftover = Some(old);
        self.manifest.schema = self.manifest.schema.max(SCHEMA_KEYCHAIN);
        self.manifest.write(&self.dir)?;
        self.remove_leftover_key(from)
    }

    /// The collection a [`Profile::move_key`] that stopped partway left a copy of the key
    /// in, [`DEFAULT_COLLECTION`] for the default one; `None` when there is none.
    pub fn leftover_key_collection(&self) -> Option<&str> {
        self.manifest.keychain_leftover.as_deref()
    }

    /// Remove the copy of the key a [`Profile::move_key`] left in `keys`, the keychain
    /// looking in [`Profile::leftover_key_collection`]. Call with the profile locked. Never
    /// removes the key the profile is on: a leftover in the collection the profile has
    /// since moved back to is that key now, and only the note of it goes.
    pub fn remove_leftover_key(&mut self, keys: &dyn KeyStore) -> Result<()> {
        let Some(leftover) = self.leftover_key_collection() else {
            return Ok(());
        };
        if leftover != self.keychain_collection().unwrap_or(DEFAULT_COLLECTION) {
            keys.delete(self.id(), Slot::Current)?;
        }
        self.manifest.keychain_leftover = None;
        self.manifest.write(&self.dir)
    }

    /// The zone Beacon runs the profile's tabs in.
    pub fn zone_id(&self) -> Uuid {
        self.manifest.zones.first().map_or(LEGACY_ZONE, |z| z.id)
    }

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

    /// `dir` as a profile, writing a manifest when it has none. A folder already holding
    /// the old flat layout keeps the old zone id, so its cookies and storage still match.
    /// A folder this makes is private; one that exists keeps its mode, since it may be
    /// shared or the user's own, and only gets a warning when others can read it.
    fn adopt(dir: &Path, cache_root: &Path, version: &str) -> Result<Self> {
        if dir.exists() {
            warn_unless_private(dir);
        } else {
            make_private_dir(dir)?;
        }
        let manifest = match Manifest::read(dir)? {
            Some(manifest) => manifest,
            None => {
                // A new manifest is a new id, and the id is key material: the databases
                // already encrypted here would open under no key after it.
                if holds_encrypted_stores(dir) {
                    bail!(
                        "{} holds encrypted files but no {MANIFEST}; restore the manifest from a backup, since a new one would give the profile an id its key does not open under",
                        dir.display()
                    );
                }
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

    /// Refuse a newer profile, make the folders private (the profile folder itself only
    /// when `own_dir`), set up the cache and record this version as the last to open it.
    fn prepare(&mut self, version: &str, own_dir: bool) -> Result<()> {
        self.manifest.ensure_understood()?;
        if own_dir {
            make_private_dir(&self.dir)?;
        }
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

    /// Whether another process holds the lock right now. A lock file that is missing or
    /// cannot be opened is no holder: a running Beacon has it open.
    pub fn in_use(&self) -> bool {
        let Ok(file) = File::options().write(true).open(self.dir.join(LOCK)) else {
            return false;
        };
        matches!(file.try_lock(), Err(fs::TryLockError::WouldBlock))
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
        self.manifest.ensure_understood()?;
        let Some(keys) = keys else {
            bail!(
                "profile {:?} cannot be encrypted: this platform has no keychain Beacon can use",
                self.name()
            );
        };
        // A rotation that stopped partway leaves the stores split across two keys; a
        // manifest rolled back to before `[crypto]` must not quietly settle on one of them.
        self.ensure_not_rotating(keys)?;
        let key = match keys.get(self.id(), Slot::Current)? {
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
        let Some(keys) = keys else {
            return Err(self.locked(crypto, false));
        };
        self.ensure_not_rotating(keys)?;
        match keys.get(self.id(), Slot::Current)? {
            Some(key) if key.check() == crypto.check => Ok(key),
            Some(_) => Err(self.locked(crypto, true)),
            None => Err(self.locked(crypto, false)),
        }
    }

    fn locked(&self, crypto: &Crypto, stale: bool) -> anyhow::Error {
        anyhow!(Locked {
            id: self.id(),
            name: self.name().to_string(),
            passphrase: crypto.passphrase.is_some(),
            stale,
        })
    }

    /// Whether a key rotation of this profile was started and not finished: until
    /// [`Profile::rotate_key`] is called again, the databases may be on either key.
    pub fn rotation_pending(&self, keys: &dyn KeyStore) -> Result<bool> {
        Ok(keys.get(self.id(), Slot::Next)?.is_some())
    }

    /// [`Rotating`] as an error when [`Profile::rotation_pending`].
    pub fn ensure_not_rotating(&self, keys: &dyn KeyStore) -> Result<()> {
        if self.rotation_pending(keys)? {
            bail!(Rotating {
                id: self.id(),
                name: self.name().to_string(),
            });
        }
        Ok(())
    }

    /// Move the profile to a new key: the databases are re-encrypted with it, the keychain
    /// holds it, and the manifest carries `passphrase` wrapped around it, or no passphrase
    /// at all. This is what changing or removing the passphrase does, since re-wrapping
    /// the same key would leave every earlier copy of `profile.toml` able to open the
    /// databases with the passphrase it carried.
    ///
    /// `rekey` is handed each store file that exists with its old and new store key, and
    /// must make it open with the new one: re-encrypt it when it opens with the old key,
    /// leave it when it already opens with the new one, and fail on one that opens with
    /// neither. The [`SEALED_FILES`] are re-sealed here, since this crate holds that cipher.
    /// The new key is filed in the keychain's [`Slot::Next`] before the first file is
    /// touched and the manifest is written before the current slot moves, so a rotation
    /// that stops partway (a crash, a failing `rekey`) loses nothing: until it is called
    /// again and finishes, every way into the profile reports [`Rotating`]. Call with the
    /// profile locked.
    pub fn rotate_key(
        &mut self,
        keys: &dyn KeyStore,
        passphrase: Option<&str>,
        params: KdfParams,
        rekey: &mut Rekey<'_>,
    ) -> Result<ProfileKey> {
        if passphrase == Some("") {
            bail!("an empty passphrase protects nothing");
        }
        // Before any store is touched: the manifest written at the end would be refused.
        self.manifest.ensure_understood()?;
        // A manifest with `[crypto]` must be one this build understands. One without is
        // only acceptable while a rotation waits to be finished: rolled back to before the
        // profile was encrypted, it gets its `[crypto]` back below, from the new key.
        let crypto = match &self.manifest.crypto {
            Some(_) => Some(self.crypto()?.clone()),
            None => None,
        };
        let next = keys.get(self.id(), Slot::Next)?;
        let Some(old) = keys.get(self.id(), Slot::Current)? else {
            return Err(match &crypto {
                Some(crypto) => self.locked(crypto, false),
                None => anyhow!("profile {:?} has no key in this machine's keychain", self.name()),
            });
        };
        let new = match next {
            // Picking up where an earlier call stopped: the two keys are in the keychain,
            // whatever the manifest says.
            Some(new) => new,
            None => {
                let Some(crypto) = &crypto else {
                    bail!("profile {:?} is not encrypted", self.name());
                };
                if old.check() != crypto.check {
                    return Err(self.locked(crypto, true));
                }
                let new = ProfileKey::generate(self.id());
                keys.set(&new, Slot::Next)?;
                let kept = keys.get(self.id(), Slot::Next)?.is_some_and(|back| back.check() == new.check());
                if !kept {
                    bail!("the keychain did not keep the new key for profile {:?}", self.name());
                }
                new
            }
        };
        for store in Store::ALL {
            let path = self.dir.join(store.file_name());
            if path.exists() {
                rekey(&path, &old.store_key(store), &new.store_key(store)).with_context(|| format!("re-encrypting {}", path.display()))?;
            }
        }
        for name in SEALED_FILES {
            self.reseal(name, &old, &new)
                .with_context(|| format!("re-encrypting {}", self.dir.join(name).display()))?;
        }
        self.manifest.crypto = Some(Crypto {
            version: CRYPTO_VERSION,
            check: new.check(),
            passphrase: passphrase.map(|p| new.wrap(p, params)).transpose()?,
        });
        self.manifest.schema = self.manifest.schema.max(SCHEMA_CRYPTO);
        self.manifest.write(&self.dir)?;
        if !file_key(keys, self.id(), &new)? {
            bail!("the keychain did not keep the key for profile {:?}", self.name());
        }
        keys.delete(self.id(), Slot::Next)?;
        Ok(new)
    }

    /// Unwrap the profile key with `passphrase` and file it in `keys`, so later runs open
    /// the profile without asking. Call with the profile locked.
    pub fn unlock_with_passphrase(&self, keys: &dyn KeyStore, passphrase: &str) -> Result<ProfileKey> {
        self.ensure_not_rotating(keys)?;
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

    /// Wrap `key` with `passphrase` into the manifest of a profile that has none yet. The
    /// profile can then be opened on another machine. Changing a passphrase is
    /// [`Profile::rotate_key`]'s: with one set already, there is a wrapped key out there to
    /// revoke, and this would not. Call with the profile locked.
    pub fn set_passphrase(&mut self, key: &ProfileKey, passphrase: &str, params: KdfParams) -> Result<()> {
        if passphrase.is_empty() {
            bail!("an empty passphrase protects nothing");
        }
        self.manifest.ensure_understood()?;
        let crypto = self.crypto()?;
        if crypto.passphrase.is_some() {
            bail!("profile {:?} has a passphrase; changing it means a new key", self.name());
        }
        if key.check() != crypto.check {
            bail!("that key is not profile {:?}'s", self.name());
        }
        let wrapped = key.wrap(passphrase, params)?;
        if let Some(crypto) = &mut self.manifest.crypto {
            crypto.passphrase = Some(wrapped);
        }
        self.manifest.write(&self.dir)
    }

    /// Whether the manifest carries a passphrase-wrapped key.
    pub fn has_passphrase(&self) -> bool {
        self.manifest.crypto.as_ref().is_some_and(|c| c.passphrase.is_some())
    }

    /// Whether anything in the folder is encrypted under a profile key.
    fn has_encrypted_store(&self) -> bool {
        holds_encrypted_stores(&self.dir)
    }
}

/// The sealed files: the small ones beside the databases, encrypted with the profile key.
impl Profile {
    /// `name`, one of [`SEALED_FILES`], decrypted; `None` when there is none. A file written
    /// before Beacon sealed these is read as it is, and the next write seals it.
    pub fn read_sealed(&self, key: &ProfileKey, name: &str) -> Result<Option<Vec<u8>>> {
        let path = self.sealed_path(name)?;
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        if crypto::is_sealed(&data) {
            return Ok(Some(
                key.unseal(name, &data).with_context(|| format!("opening {}", path.display()))?,
            ));
        }
        Ok(Some(data))
    }

    /// Write `name`, one of [`SEALED_FILES`], sealed with `key`.
    pub fn write_sealed(&self, key: &ProfileKey, name: &str, plaintext: &[u8]) -> Result<()> {
        write_atomic(&self.sealed_path(name)?, &key.seal(name, plaintext)?)
    }

    /// Where `name` lives; an error for a name not in [`SEALED_FILES`], so no caller reaches
    /// outside the profile folder or over one of its databases.
    fn sealed_path(&self, name: &str) -> Result<PathBuf> {
        if !SEALED_FILES.contains(&name) {
            bail!("{name:?} is not one of the profile's sealed files");
        }
        Ok(self.dir.join(name))
    }

    /// Seal every one of [`SEALED_FILES`] still in plaintext: what a profile holds when it
    /// gets its key, or what a Beacon from before sealing wrote. Safe to run on every start.
    pub fn seal_plaintext_files(&self, key: &ProfileKey) -> Result<()> {
        for name in SEALED_FILES {
            let path = self.dir.join(name);
            match fs::read(&path) {
                Ok(data) if !crypto::is_sealed(&data) => write_atomic(&path, &key.seal(name, &data)?)?,
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
            }
        }
        Ok(())
    }

    /// Move `name` from `old` to `new` for [`Profile::rotate_key`]: nothing when it is
    /// missing or already on `new`; a plaintext one is sealed on the way.
    fn reseal(&self, name: &str, old: &ProfileKey, new: &ProfileKey) -> Result<()> {
        let path = self.dir.join(name);
        let data = match fs::read(&path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        let plaintext = if crypto::is_sealed(&data) {
            if new.unseal(name, &data).is_ok() {
                return Ok(());
            }
            old.unseal(name, &data)?
        } else {
            data
        };
        write_atomic(&path, &new.seal(name, &plaintext)?)
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

/// What [`Profile::rotate_key`] re-encrypts one store file with: the file, the store key it
/// is on and the one it must end up on. See there for what it has to do.
pub type Rekey<'a> = dyn FnMut(&Path, &Zeroizing<[u8; 32]>, &Zeroizing<[u8; 32]>) -> Result<()> + 'a;

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
            write!(
                f,
                "profile {:?} is encrypted and {held}; unlock it with `beaconctl profile unlock {}`",
                self.name,
                shell_arg(&self.name, self.id)
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

/// A key rotation ([`Profile::rotate_key`]) that was started and not finished. The new key
/// is in the keychain's [`Slot::Next`], and the databases may be on either key; calling
/// `rotate_key` again finishes it. Find it with `err.downcast_ref::<Rotating>()`.
#[derive(Debug)]
pub struct Rotating {
    pub id: Uuid,
    pub name: String,
}

impl std::fmt::Display for Rotating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "profile {:?} is partway through a key rotation that was interrupted; run `beaconctl profile passphrase {}` (or `remove-passphrase`) again to finish it",
            self.name,
            shell_arg(&self.name, self.id)
        )
    }
}

impl std::error::Error for Rotating {}

/// How a hint names the profile: the name when it can be pasted into a shell as it is, the
/// id otherwise.
fn shell_arg(name: &str, id: Uuid) -> String {
    let plain = !name.starts_with('-') && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if plain {
        name.to_string()
    } else {
        id.to_string()
    }
}

/// File `key` in `keys` as the profile's current key and read it back: a keychain that
/// accepts a key and loses it would leave databases nothing can open. `Ok(false)` when it
/// did not keep it.
fn file_key(keys: &dyn KeyStore, profile: Uuid, key: &ProfileKey) -> Result<bool> {
    keys.set(key, Slot::Current)?;
    Ok(keys.get(profile, Slot::Current)?.is_some_and(|back| back.check() == key.check()))
}

/// Held for as long as a process uses a profile.
#[derive(Debug)]
pub struct ProfileLock {
    _file: File,
}

fn holds_legacy_files(dir: &Path) -> bool {
    LEGACY_FILES.iter().any(|name| dir.join(name).exists())
}

/// Whether anything in `dir` is encrypted under the profile key: a store that is not
/// plaintext SQLite, or a sealed small file. Judged by the file header alone, as the engine
/// judges what to encrypt: this crate does not open databases.
fn holds_encrypted_stores(dir: &Path) -> bool {
    Store::ALL.iter().any(|store| is_encrypted_database(&dir.join(store.file_name())))
        || SEALED_FILES.iter().any(|name| is_sealed_file(&dir.join(name)))
}

/// Whether `path` starts with the sealed-file magic. Only the magic is read: a leftover
/// file can be any size.
fn is_sealed_file(path: &Path) -> bool {
    use std::io::Read;
    let mut magic = [0u8; crypto::SEALED_MAGIC.len()];
    File::open(path).and_then(|mut f| f.read_exact(&mut magic)).is_ok() && crypto::is_sealed(&magic)
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

/// Create `dir` and make it readable by the owner only: a profile's files are in plain text
/// until its first keyed start encrypts them, and the lock and manifest always are; the
/// folder mode is what keeps other users out of every file inside.
fn make_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).with_context(|| format!("setting the mode of {}", dir.display()))?;
    }
    Ok(())
}

/// Log when users other than the owner can get into `dir`.
fn warn_unless_private(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(dir) {
            if meta.permissions().mode() & 0o077 != 0 {
                log::warn!(
                    "{} is open to other users (mode {:o}); its lock and manifest are readable to them",
                    dir.display(),
                    meta.permissions().mode() & 0o777
                );
            }
        }
    }
    #[cfg(not(unix))]
    let _ = dir;
}

/// Write via a temp file and rename, so a crash leaves the old file or the new one.
fn write_atomic(path: &Path, data: impl AsRef<[u8]>) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, data).with_context(|| format!("writing {}", tmp.display()))?;
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

    #[cfg(unix)]
    #[test]
    fn an_exact_dir_that_exists_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let mode = |dir: &Path| fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        let shared = tmp.path().join("shared");
        fs::create_dir_all(&shared).unwrap();
        fs::set_permissions(&shared, fs::Permissions::from_mode(0o755)).unwrap();
        roots.open(&Selection::Dir(shared.clone()), V).unwrap();
        roots.open(&Selection::Dir(shared.clone()), V).unwrap();
        assert_eq!(mode(&shared), 0o755);

        let fresh = tmp.path().join("fresh");
        roots.open(&Selection::Dir(fresh.clone()), V).unwrap();
        assert_eq!(mode(&fresh), 0o700);
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
        // This build will not write such a manifest; a newer one did.
        assert!(p.manifest.write(&p.dir).is_err());
        let newer = toml::to_string_pretty(&p.manifest).unwrap();
        fs::write(p.dir.join(MANIFEST), &newer).unwrap();
        let err = roots.open(&Selection::Default, V).unwrap_err();
        assert!(err.to_string().contains("newer Beacon"), "{err}");
        // Listed and read as it is, but nothing that would write it back goes ahead.
        let mut listed = roots.list().unwrap().remove(0);
        let err = listed.unlock(Some(&crypto::MemoryKeyStore::default())).unwrap_err();
        assert!(err.to_string().contains("newer Beacon"), "{err}");
        let mut at = Profile::at(&p.dir, &roots.cache).unwrap();
        let err = at
            .rotate_key(&crypto::MemoryKeyStore::default(), None, crypto::TEST_KDF, &mut |_, _, _| Ok(()))
            .unwrap_err();
        assert!(err.to_string().contains("newer Beacon"), "{err}");
        assert_eq!(fs::read_to_string(p.dir.join(MANIFEST)).unwrap(), newer);
    }

    #[test]
    fn a_profile_made_before_the_first_start_does_not_block_migration() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        fs::create_dir_all(&roots.data).unwrap();
        fs::write(roots.data.join("cookies.db"), "c").unwrap();
        // `beaconctl profile create` before Beacon ever ran under profiles.
        let work = roots.create("work", V).unwrap();
        assert!(!roots.data.join("cookies.db").exists());
        let p = roots.open(&Selection::Default, V).unwrap();
        assert_ne!(p.id(), work.id());
        assert_eq!(p.zone_id(), LEGACY_ZONE);
        assert_eq!(fs::read_to_string(p.dir.join("cookies.db")).unwrap(), "c");
        assert_eq!(roots.default_id().unwrap(), Some(p.id()));
    }

    #[test]
    fn a_migration_that_stopped_partway_finishes_into_the_same_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        fs::create_dir_all(&roots.data).unwrap();
        fs::write(roots.data.join("cookies.db"), "c").unwrap();
        let p = roots.open(&Selection::Default, V).unwrap();
        // As if the move had stopped before this file, or an old Beacon ran again since.
        fs::write(roots.data.join("places.db"), "p").unwrap();
        fs::write(roots.data.join("cookies.db"), "older").unwrap();
        let again = roots.open(&Selection::Named("default".into()), V).unwrap();
        assert_eq!(again.id(), p.id());
        assert_eq!(roots.list().unwrap().len(), 1);
        assert_eq!(fs::read_to_string(p.dir.join("places.db")).unwrap(), "p");
        // The profile's own copy is kept over the stray one.
        assert_eq!(fs::read_to_string(p.dir.join("cookies.db")).unwrap(), "c");
    }

    #[test]
    fn a_migration_that_stopped_before_the_default_still_becomes_it() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        fs::create_dir_all(&roots.data).unwrap();
        fs::write(roots.data.join("cookies.db"), "c").unwrap();
        let p = roots.open(&Selection::Default, V).unwrap();
        // As if it had stopped after the profile was placed, before `profiles.toml`.
        fs::remove_file(roots.data.join(DEFAULTS)).unwrap();
        fs::write(roots.data.join("places.db"), "p").unwrap();
        let work = roots.create("work", V).unwrap();
        assert_eq!(roots.default_id().unwrap(), Some(p.id()));
        assert_ne!(work.id(), p.id());
    }

    #[test]
    fn a_folder_with_sealed_files_and_no_manifest_is_not_adopted() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let dir = tmp.path().join("copied");
        fs::create_dir_all(&dir).unwrap();
        let mut sealed = crypto::SEALED_MAGIC.to_vec();
        sealed.extend_from_slice(&[7; 40]);
        fs::write(dir.join("session.tabs"), sealed).unwrap();
        let err = roots.open(&Selection::Dir(dir.clone()), V).unwrap_err();
        assert!(err.to_string().contains("encrypted files"), "{err}");
        assert!(!dir.join(MANIFEST).exists());
    }

    #[test]
    fn opening_waits_for_another_process_picking_a_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let held = roots.lock().unwrap();
        let other = roots.clone();
        let opener = std::thread::spawn(move || other.open(&Selection::Default, V).unwrap().id());
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!opener.is_finished());
        assert!(roots.list().unwrap().is_empty());
        drop(held);
        let id = opener.join().unwrap();
        assert_eq!(roots.open(&Selection::Default, V).unwrap().id(), id);
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
    fn only_a_held_lock_is_in_use() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        assert!(!p.in_use(), "no lock file yet");
        let held = p.lock().unwrap();
        assert!(p.in_use());
        drop(held);
        assert!(!p.in_use());
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
    fn a_profile_in_use_is_left_as_it_is() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let p = roots.open(&Selection::Default, V).unwrap();
        let _held = p.lock().unwrap();
        fs::write(p.cache_dir.join("VERSION"), "0").unwrap();
        fs::write(p.cache_dir.join("entry"), "x").unwrap();
        roots.open(&Selection::Default, "9.9.9").unwrap();
        assert!(p.cache_dir.join("entry").exists());
        assert_eq!(Manifest::read(&p.dir).unwrap().unwrap().last_opened_by, V);
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

    #[test]
    fn a_key_moved_to_another_collection_opens_from_there_only() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let login = crypto::MemoryKeyStore::default();
        let private = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&login)).unwrap();

        p.move_key(&login, &private, Some("private")).unwrap();
        assert!(login.get(p.id(), Slot::Current).unwrap().is_none());
        let again = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(again.keychain_collection(), Some("private"));
        assert_eq!(again.key_from(Some(&private)).unwrap().check(), key.check());
        // A Beacon from before collections would drop the field and look in the login one.
        assert_eq!(again.manifest.schema, SCHEMA_KEYCHAIN);
    }

    /// A keychain that holds keys but will not let go of them.
    struct Clinging<'a>(&'a crypto::MemoryKeyStore);

    impl KeyStore for Clinging<'_> {
        fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
            self.0.get(profile, slot)
        }

        fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()> {
            self.0.set(key, slot)
        }

        fn delete(&self, _profile: Uuid, _slot: Slot) -> Result<()> {
            bail!("keychain locked")
        }
    }

    #[test]
    fn a_move_that_could_not_remove_the_old_copy_is_finished_later() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let login = crypto::MemoryKeyStore::default();
        let private = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        p.unlock(Some(&login)).unwrap();

        assert!(p.move_key(&Clinging(&login), &private, Some("private")).is_err());
        let mut again = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(again.keychain_collection(), Some("private"));
        assert_eq!(again.leftover_key_collection(), Some(DEFAULT_COLLECTION));
        again.key_from(Some(&private)).unwrap();
        // A second move would lose track of the first copy.
        assert!(again.move_key(&private, &login, None).is_err());

        again.remove_leftover_key(&login).unwrap();
        assert!(login.get(again.id(), Slot::Current).unwrap().is_none());
        let done = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(done.leftover_key_collection(), None);
        done.key_from(Some(&private)).unwrap();
    }

    #[test]
    fn a_leftover_in_the_collection_the_profile_is_on_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let login = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        p.unlock(Some(&login)).unwrap();
        p.manifest.keychain_leftover = Some(DEFAULT_COLLECTION.to_string());

        p.remove_leftover_key(&login).unwrap();
        assert_eq!(p.leftover_key_collection(), None);
        p.key_from(Some(&login)).unwrap();
    }

    #[test]
    fn a_profile_without_a_key_only_records_its_collection() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let login = crypto::MemoryKeyStore::default();
        let private = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();

        p.move_key(&login, &private, Some("private")).unwrap();
        let mut again = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(again.keychain_collection(), Some("private"));
        assert!(again.manifest.crypto.is_none());
        again.unlock(Some(&private)).unwrap();
        assert!(private.get(again.id(), Slot::Current).unwrap().is_some());
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
        assert_eq!(keys.get(p.id(), Slot::Current).unwrap().unwrap().check(), key.check());
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
        assert!(there.get(p.id(), Slot::Current).unwrap().is_none());

        // Plaintext databases, or none, are what a new key is for.
        fs::write(p.dir.join(Store::Places.file_name()), b"SQLite format 3\0 and a page of plaintext").unwrap();
        fs::write(p.dir.join(Store::Cookies.file_name()), b"").unwrap();
        let mut copy = roots.open(&Selection::Default, V).unwrap();
        copy.unlock(Some(&there)).unwrap();
        assert!(there.get(p.id(), Slot::Current).unwrap().is_some());
    }

    /// Stands in for SQLCipher: a store file holds the hex of the store key it is
    /// "encrypted" with, and re-encrypting it swaps that for the new one.
    fn fake_rekey(path: &Path, old: &Zeroizing<[u8; 32]>, new: &Zeroizing<[u8; 32]>) -> Result<()> {
        let content = fs::read_to_string(path)?;
        if content == crypto::hex(new.as_slice()) {
            return Ok(());
        }
        if content == crypto::hex(old.as_slice()) {
            fs::write(path, crypto::hex(new.as_slice()))?;
            return Ok(());
        }
        bail!("{} opens with neither key", path.display())
    }

    fn seal_fake_stores(p: &Profile, key: &ProfileKey) {
        for store in Store::ALL {
            fs::write(p.dir.join(store.file_name()), crypto::hex(key.store_key(store).as_slice())).unwrap();
        }
    }

    fn fake_stores_open_with(p: &Profile, key: &ProfileKey) -> bool {
        Store::ALL
            .iter()
            .all(|store| fs::read_to_string(p.dir.join(store.file_name())).unwrap() == crypto::hex(key.store_key(*store).as_slice()))
    }

    #[test]
    fn changing_or_removing_the_passphrase_moves_the_profile_to_a_new_key() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let first = p.unlock(Some(&keys)).unwrap();
        seal_fake_stores(&p, &first);
        p.set_passphrase(&first, "old", crypto::TEST_KDF).unwrap();
        // A second passphrase is a new key, never a re-wrap of the first.
        assert!(p.set_passphrase(&first, "other", crypto::TEST_KDF).is_err());

        let second = p.rotate_key(&keys, Some("new"), crypto::TEST_KDF, &mut fake_rekey).unwrap();
        assert_ne!(second.check(), first.check());
        assert!(fake_stores_open_with(&p, &second));
        let written = Manifest::read(&p.dir).unwrap().unwrap();
        assert_eq!(written.crypto.as_ref().unwrap().check, second.check());
        // The old passphrase, from an old copy of the manifest, opens nothing now.
        assert!(p.key_from_passphrase("old").is_err());
        assert_eq!(p.key_from_passphrase("new").unwrap().check(), second.check());
        assert_eq!(keys.get(p.id(), Slot::Current).unwrap().unwrap().check(), second.check());
        assert!(keys.get(p.id(), Slot::Next).unwrap().is_none());
        assert_eq!(p.unlock(Some(&keys)).unwrap().check(), second.check());

        // Removing the passphrase rotates too, for the same reason.
        let third = p.rotate_key(&keys, None, crypto::TEST_KDF, &mut fake_rekey).unwrap();
        assert_ne!(third.check(), second.check());
        assert!(fake_stores_open_with(&p, &third));
        assert!(!p.has_passphrase());
        assert!(p.key_from_passphrase("new").is_err());
        assert_eq!(p.unlock(Some(&keys)).unwrap().check(), third.check());
        assert!(p.rotate_key(&keys, Some(""), crypto::TEST_KDF, &mut fake_rekey).is_err());
    }

    #[test]
    fn an_interrupted_rotation_blocks_the_profile_until_it_is_finished() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let first = p.unlock(Some(&keys)).unwrap();
        seal_fake_stores(&p, &first);
        p.set_passphrase(&first, "pw", crypto::TEST_KDF).unwrap();

        // The second store fails halfway: one store is on the new key, three on the old.
        let mut calls = 0;
        let mut failing = |path: &Path, old: &Zeroizing<[u8; 32]>, new: &Zeroizing<[u8; 32]>| {
            calls += 1;
            if calls == 2 {
                bail!("disk full");
            }
            fake_rekey(path, old, new)
        };
        let err = p.rotate_key(&keys, Some("pw2"), crypto::TEST_KDF, &mut failing).unwrap_err();
        assert!(format!("{err:#}").contains("disk full"), "{err:#}");
        let next = keys.get(p.id(), Slot::Next).unwrap().expect("the new key waits in the next slot");
        assert_eq!(Manifest::read(&p.dir).unwrap().unwrap().crypto.unwrap().check, first.check());
        assert!(!fake_stores_open_with(&p, &first) && !fake_stores_open_with(&p, &next));

        // Nothing opens it in that state, and the error says what to do.
        let err = p.key_from(Some(&keys)).unwrap_err();
        assert!(err.downcast_ref::<Rotating>().is_some(), "{err}");
        assert!(p.unlock(Some(&keys)).unwrap_err().downcast_ref::<Rotating>().is_some());
        assert!(p
            .unlock_with_passphrase(&keys, "pw")
            .unwrap_err()
            .downcast_ref::<Rotating>()
            .is_some());

        // Trying again finishes with the key that was waiting, whatever passphrase is given now.
        let done = p.rotate_key(&keys, None, crypto::TEST_KDF, &mut fake_rekey).unwrap();
        assert_eq!(done.check(), next.check());
        assert!(fake_stores_open_with(&p, &done));
        assert!(keys.get(p.id(), Slot::Next).unwrap().is_none());
        assert_eq!(p.unlock(Some(&keys)).unwrap().check(), done.check());
        assert!(!p.has_passphrase());
    }

    #[test]
    fn a_folder_whose_manifest_is_gone_is_not_given_a_new_id() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let dir = tmp.path().join("copied");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(Store::Places.file_name()), b"not SQLite at all, sixteen bytes and more").unwrap();
        let err = roots.open(&Selection::Dir(dir.clone()), V).unwrap_err();
        assert!(err.to_string().contains(MANIFEST), "{err}");
        assert!(!dir.join(MANIFEST).exists());
    }

    #[test]
    fn the_small_files_are_sealed_and_plaintext_ones_still_read() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&keys)).unwrap();
        assert_eq!(p.read_sealed(&key, "session.tabs").unwrap(), None);

        p.write_sealed(&key, "session.tabs", b"*https://example.org/\n").unwrap();
        let on_disk = fs::read(p.dir.join("session.tabs")).unwrap();
        assert!(crypto::is_sealed(&on_disk) && !on_disk.windows(7).any(|w| w == b"example"));
        assert_eq!(p.read_sealed(&key, "session.tabs").unwrap().unwrap(), b"*https://example.org/\n");
        assert!(p.read_sealed(&ProfileKey::generate(p.id()), "session.tabs").is_err());
        // Only the sealed files: no other name, and nothing outside the folder.
        for name in ["places.db", "../escape", "/tmp/escape"] {
            assert!(p.write_sealed(&key, name, b"x").is_err());
            assert!(p.read_sealed(&key, name).is_err());
        }
        assert!(!tmp.path().join("escape").exists());

        // A file from before sealing reads as it is, and the first keyed start seals it.
        fs::write(p.dir.join("picker-clock.txt"), "12\n").unwrap();
        assert_eq!(p.read_sealed(&key, "picker-clock.txt").unwrap().unwrap(), b"12\n");
        p.seal_plaintext_files(&key).unwrap();
        assert!(crypto::is_sealed(&fs::read(p.dir.join("picker-clock.txt")).unwrap()));
        assert_eq!(p.read_sealed(&key, "picker-clock.txt").unwrap().unwrap(), b"12\n");
        // Sealed already: left as it is.
        let before = fs::read(p.dir.join("session.tabs")).unwrap();
        p.seal_plaintext_files(&key).unwrap();
        assert_eq!(fs::read(p.dir.join("session.tabs")).unwrap(), before);
    }

    #[test]
    fn a_rotation_reseals_the_small_files() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let first = p.unlock(Some(&keys)).unwrap();
        seal_fake_stores(&p, &first);
        p.set_passphrase(&first, "pw", crypto::TEST_KDF).unwrap();
        p.write_sealed(&first, "session.tabs", b"*https://example.org/\n").unwrap();
        fs::write(p.dir.join("picker-swatches.txt"), "#ff0000\n").unwrap();

        let second = p.rotate_key(&keys, None, crypto::TEST_KDF, &mut fake_rekey).unwrap();
        assert!(p.read_sealed(&first, "session.tabs").is_err());
        assert_eq!(p.read_sealed(&second, "session.tabs").unwrap().unwrap(), b"*https://example.org/\n");
        assert!(crypto::is_sealed(&fs::read(p.dir.join("picker-swatches.txt")).unwrap()));
        assert_eq!(p.read_sealed(&second, "picker-swatches.txt").unwrap().unwrap(), b"#ff0000\n");
        assert_eq!(p.read_sealed(&second, "picker-clock.txt").unwrap(), None);
    }

    #[test]
    fn a_rolled_back_manifest_does_not_settle_an_interrupted_rotation() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let first = p.unlock(Some(&keys)).unwrap();
        seal_fake_stores(&p, &first);
        p.set_passphrase(&first, "pw", crypto::TEST_KDF).unwrap();
        let mut failing = |_: &Path, _: &Zeroizing<[u8; 32]>, _: &Zeroizing<[u8; 32]>| -> Result<()> { bail!("disk full") };
        p.rotate_key(&keys, Some("pw2"), crypto::TEST_KDF, &mut failing).unwrap_err();
        // The manifest from before encryption comes back while the next key still waits.
        roll_back_manifest(&p);
        let mut again = roots.open(&Selection::Default, V).unwrap();
        let err = again.unlock(Some(&keys)).unwrap_err();
        assert!(err.downcast_ref::<Rotating>().is_some(), "{err}");
        assert!(Manifest::read(&p.dir).unwrap().unwrap().crypto.is_none());

        // The retry still gets through: both keys are in the keychain, and the manifest
        // gets its `[crypto]` back from the new one.
        let next = keys.get(p.id(), Slot::Next).unwrap().unwrap();
        let done = again.rotate_key(&keys, Some("pw3"), crypto::TEST_KDF, &mut fake_rekey).unwrap();
        assert_eq!(done.check(), next.check());
        assert!(fake_stores_open_with(&p, &done));
        let written = Manifest::read(&p.dir).unwrap().unwrap();
        assert_eq!(
            (written.schema, written.crypto.as_ref().map(|c| c.check.clone())),
            (SCHEMA_CRYPTO, Some(done.check()))
        );
        assert!(keys.get(p.id(), Slot::Next).unwrap().is_none());
        let mut opened = roots.open(&Selection::Default, V).unwrap();
        assert_eq!(opened.unlock(Some(&keys)).unwrap().check(), done.check());
        assert_eq!(opened.key_from_passphrase("pw3").unwrap().check(), done.check());
    }

    #[test]
    fn a_rotation_needs_the_profiles_own_key_here() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&keys)).unwrap();
        seal_fake_stores(&p, &key);
        keys.set(&ProfileKey::generate(p.id()), Slot::Current).unwrap();
        let err = p.rotate_key(&keys, Some("pw"), crypto::TEST_KDF, &mut fake_rekey).unwrap_err();
        assert!(err.downcast_ref::<Locked>().unwrap().stale);
        assert!(fake_stores_open_with(&p, &key));
        assert!(keys.get(p.id(), Slot::Next).unwrap().is_none());
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
            fn get(&self, _profile: Uuid, _slot: Slot) -> Result<Option<ProfileKey>> {
                Ok(None)
            }
            fn set(&self, _key: &ProfileKey, _slot: Slot) -> Result<()> {
                Ok(())
            }
            fn delete(&self, _profile: Uuid, _slot: Slot) -> Result<()> {
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
        keys.set(&ProfileKey::generate(p.id()), Slot::Current).unwrap();
        let err = p.unlock(Some(&keys)).unwrap_err();
        assert!(err.downcast_ref::<Locked>().unwrap().stale);
    }

    #[test]
    fn an_empty_passphrase_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let roots = roots(&tmp);
        let keys = crypto::MemoryKeyStore::default();
        let mut p = roots.open(&Selection::Default, V).unwrap();
        let key = p.unlock(Some(&keys)).unwrap();
        assert!(p.set_passphrase(&key, "", crypto::TEST_KDF).is_err());
        assert!(!p.has_passphrase());
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
