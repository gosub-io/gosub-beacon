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
//! The engine never sees any of this: Beacon opens the stores in the profile folder and
//! hands the engine the handles and the zone id. This crate has no engine or toolkit
//! dependency, so `beaconctl` can share it.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// The newest manifest layout this build understands. A profile written by a newer Beacon
/// is refused rather than opened, so an older build cannot quietly damage it.
pub const SCHEMA: u32 = 1;

/// The zone id every Beacon before profiles used, compiled in. A profile adopted from that
/// layout keeps it, or its cookies and localStorage rows would no longer match.
pub const LEGACY_ZONE: Uuid = Uuid::from_u128(0xf1234567_abcd_4000_8000_000000000001);

/// Bumped when the cache layout changes; a cache with another stamp is thrown away.
const CACHE_VERSION: &str = "1";

const MANIFEST: &str = "profile.toml";
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
            schema: SCHEMA,
            id: Uuid::new_v4(),
            name: name.to_string(),
            created_by: version.to_string(),
            last_opened_by: version.to_string(),
            zones: vec![ZoneEntry {
                id: zone,
                name: "Default".to_string(),
            }],
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

/// Held for as long as a process uses a profile.
#[derive(Debug)]
pub struct ProfileLock {
    _file: File,
}

fn holds_legacy_files(dir: &Path) -> bool {
    LEGACY_FILES.iter().any(|name| dir.join(name).exists())
}

/// Create `dir` and make it readable by the owner only: a profile holds cookies in plain
/// text, and the folder mode is what keeps other users out of every file inside.
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
