//! A profile as one file, to carry to another machine: a gzip-compressed tar.
//!
//! ```text
//! beacon-export.toml        always first: format, profile id and name, and every file
//!                           below with its size and sha256
//! profile/profile.toml      the manifest, without this machine's keychain fields
//! profile/settings.db ...   the stores, as they are on disk (encrypted), with any WAL
//! profile/session.tabs ...  the sealed files
//! profile/local-storage/... the isolated build's localStorage areas
//! ```
//!
//! An export only ever holds an encrypted profile, and its key goes along wrapped by a
//! passphrase: the profile's own, or one given for the archive alone. The lock, the
//! SQLite `-shm` index, stale files and half-written `.tmp` files stay behind, and the cache
//! was never in the folder.
//!
//! An import treats the archive as hostile: it takes only the files an export writes,
//! under the names an export gives them, checks every one against the list in
//! `beacon-export.toml`, and puts nothing in place until all of it checked out.

use super::*;
use crypto::KdfParams;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};

/// The archive layout this build writes and reads. A newer one is refused.
pub const EXPORT_FORMAT: u32 = 1;
/// The first entry of every archive.
pub const EXPORT_MANIFEST: &str = "beacon-export.toml";
/// The folder in the archive that holds the profile.
const PREFIX: &str = "profile";
/// The isolated build's localStorage: one file per area, named by a hash.
const LOCAL_STORAGE_DIR: &str = "local-storage";

/// The most an import unpacks. Far above any real profile; a crafted archive cannot fill
/// the disk with much more.
const MAX_IMPORT_BYTES: u64 = 8 << 30;
/// The most files an import unpacks.
const MAX_FILES: usize = 100_000;
/// The most `beacon-export.toml` may be: a line per file, [`MAX_FILES`] of them.
const MAX_LIST_BYTES: u64 = 32 << 20;
/// The most `profile.toml` may be.
const MAX_MANIFEST_BYTES: u64 = 1 << 20;

/// Where an import unpacks before it is put in place, and where a replaced profile waits
/// until its successor is. Both are dot folders, which [`Roots::list`] skips.
const STAGING: &str = ".import-";
const REPLACED: &str = ".replaced-";

/// `beacon-export.toml`.
#[derive(Debug, Serialize, Deserialize)]
struct Contents {
    format: u32,
    /// The Beacon version that wrote the archive.
    created_by: String,
    /// Seconds since the Unix epoch.
    exported_at: u64,
    id: Uuid,
    name: String,
    #[serde(rename = "file", default)]
    files: Vec<Listed>,
}

/// One file in the archive, its path relative to `profile/`.
#[derive(Debug, Serialize, Deserialize)]
struct Listed {
    path: String,
    size: u64,
    sha256: String,
}

impl Profile {
    /// Write the profile to `out` as an archive. Call with the profile locked, so nothing
    /// writes to it meanwhile.
    ///
    /// The profile must be encrypted, every store and sealed file of it. A profile with a
    /// passphrase carries its own wrapped key; one without needs `wrap`, its key and a
    /// passphrase to wrap it with in the archive's copy of the manifest only. Whoever has
    /// the archive and that passphrase has the key this machine opens the profile with.
    pub fn export(&self, out: impl Write, wrap: Option<(&ProfileKey, &str)>, version: &str) -> Result<()> {
        self.manifest.ensure_understood()?;
        if self.manifest.crypto.is_none() {
            bail!(
                "profile {:?} is not encrypted yet; start Beacon with it once, which encrypts it",
                self.name()
            );
        }
        let crypto = self.crypto()?;
        let mut manifest = self.manifest.clone();
        // Which keychain collection holds the key says something about this machine only.
        manifest.keychain = None;
        manifest.keychain_leftover = None;
        if manifest.schema == SCHEMA_KEYCHAIN {
            manifest.schema = SCHEMA_CRYPTO;
        }
        if crypto.passphrase.is_none() {
            let Some((key, passphrase)) = wrap else {
                bail!(
                    "profile {:?} has no passphrase, and its archive needs one to carry the key",
                    self.name()
                );
            };
            if passphrase.is_empty() {
                bail!("an empty passphrase protects nothing");
            }
            if key.check() != crypto.check {
                bail!("that key is not profile {:?}'s", self.name());
            }
            if let Some(crypto) = &mut manifest.crypto {
                crypto.passphrase = Some(key.wrap(passphrase, KdfParams::default())?);
            }
        }
        let manifest_text = toml::to_string_pretty(&manifest)?;

        let files = self.exported_files()?;
        let mut listed = vec![Listed {
            path: MANIFEST.to_string(),
            size: manifest_text.len() as u64,
            sha256: hex(&Sha256::digest(manifest_text.as_bytes())),
        }];
        for rel in &files {
            let path = self.dir.join(rel);
            let (size, sha256) = hash_file(&path)?;
            listed.push(Listed {
                path: rel.clone(),
                size,
                sha256,
            });
        }
        let exported_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let contents = Contents {
            format: EXPORT_FORMAT,
            created_by: version.to_string(),
            exported_at,
            id: self.id(),
            name: self.name().to_string(),
            files: listed,
        };

        let mut tar = tar::Builder::new(GzEncoder::new(out, flate2::Compression::default()));
        let contents_text = toml::to_string_pretty(&contents)?;
        append(&mut tar, EXPORT_MANIFEST, contents_text.as_bytes(), exported_at)?;
        append(&mut tar, &format!("{PREFIX}/{MANIFEST}"), manifest_text.as_bytes(), exported_at)?;
        // The list was made from the files a moment ago; with the profile locked they have
        // not changed, and one that did anyway fails the export rather than the import.
        for item in &contents.files[1..] {
            let path = self.dir.join(&item.path);
            let file = File::open(&path).with_context(|| format!("reading {}", path.display()))?;
            let mut reader = Hashing::new(file.take(item.size));
            let mut header = header(item.size, exported_at);
            tar.append_data(&mut header, format!("{PREFIX}/{}", item.path), &mut reader)
                .with_context(|| format!("archiving {}", path.display()))?;
            if reader.len != item.size || reader.digest() != item.sha256 {
                bail!("{} changed while it was exported", path.display());
            }
        }
        tar.into_inner()?.finish()?.flush()?;
        Ok(())
    }

    /// The files an export takes, relative to the profile folder, and only once each is
    /// known to be encrypted.
    fn exported_files(&self) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for store in Store::ALL {
            let name = store.file_name();
            if regular_file(&self.dir.join(name))? {
                if is_plaintext_database(&self.dir.join(name)) {
                    bail!(
                        "{name} in profile {:?} is not encrypted yet; start Beacon with it once, which encrypts it",
                        self.name()
                    );
                }
                out.push(name.to_string());
            }
            // Pages a Beacon that stopped short never moved into the database: part of it.
            let wal = format!("{name}-wal");
            if regular_file(&self.dir.join(&wal))? {
                out.push(wal);
            }
        }
        for name in SEALED_FILES {
            let path = self.dir.join(name);
            if regular_file(&path)? {
                if fs::metadata(&path)?.len() > 0 && !is_sealed_file(&path) {
                    bail!(
                        "{name} in profile {:?} is not sealed yet; start Beacon with it once, which seals it",
                        self.name()
                    );
                }
                out.push(name.to_string());
            }
        }
        let areas = self.dir.join(LOCAL_STORAGE_DIR);
        match fs::symlink_metadata(&areas) {
            Ok(meta) if meta.is_dir() => {
                let mut names = Vec::new();
                for entry in fs::read_dir(&areas)? {
                    let entry = entry?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    // The engine's own half-written files and anything else are not areas.
                    if is_area_name(&name) && entry.file_type()?.is_file() {
                        names.push(format!("{LOCAL_STORAGE_DIR}/{name}"));
                    } else {
                        log::warn!("{} is not a localStorage area; left out of the export", entry.path().display());
                    }
                }
                names.sort();
                out.extend(names);
            }
            Ok(_) => bail!("{} is not a folder", areas.display()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", areas.display())),
        }
        Ok(out)
    }
}

impl Roots {
    /// Unpack an archive [`Profile::export`] wrote into a profile here, under the id it
    /// had. A name another profile has is refused; `name` gives it another. A profile with
    /// the same id is refused unless `replace`, which puts the archive's in its place
    /// (refused while Beacon has it open) and keeps where this machine's keychain holds its
    /// key. The first profile on the machine becomes the default.
    ///
    /// The profile comes in locked: its key is in the archive only, wrapped by the
    /// passphrase, and [`Profile::unlock_with_passphrase`] files it here.
    pub fn import(&self, archive: impl Read, name: Option<&str>, replace: bool, version: &str) -> Result<Profile> {
        let _lock = self.lock()?;
        make_private_dir(&self.profiles_dir())?;
        let staging = self.profiles_dir().join(format!("{STAGING}{}", Uuid::new_v4()));
        make_private_dir(&staging)?;
        let result = self.import_into(&staging, archive, name, replace, version);
        if staging.exists() {
            if let Err(e) = fs::remove_dir_all(&staging) {
                log::warn!("removing {}: {e}", staging.display());
            }
        }
        result
    }

    fn import_into(&self, staging: &Path, archive: impl Read, name: Option<&str>, replace: bool, version: &str) -> Result<Profile> {
        let (contents, manifest_bytes) = unpack(archive, staging)?;
        let text = std::str::from_utf8(&manifest_bytes).context("the archive's profile.toml is not text")?;
        let mut manifest: Manifest = toml::from_str(text).context("parsing the archive's profile.toml")?;
        manifest.ensure_understood()?;
        if manifest.id != contents.id {
            bail!("the archive lists profile {} but holds {}", contents.id, manifest.id);
        }
        let imported = Profile {
            dir: staging.to_path_buf(),
            cache_dir: self.cache.join(manifest.id.to_string()),
            manifest: manifest.clone(),
        };
        // Checks the scheme version as well.
        let crypto = imported.crypto().ok();
        let Some((check, wrapped)) = crypto.and_then(|c| Some((&c.check, c.passphrase.as_ref()?))) else {
            bail!("the archive holds no passphrase-wrapped key, so nothing could open the profile in it");
        };
        // What `ProfileKey::check` writes: 16 bytes in lowercase hex.
        if check.len() != 32 || !check.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            bail!("the archive's key check is damaged, so nothing could open the profile in it");
        }
        // Before it goes anywhere: found out at unlock, a broken key has replaced a good one.
        wrapped
            .validate()
            .context("the archive's passphrase-wrapped key is damaged, so nothing could open the profile in it")?;
        manifest.keychain = None;
        manifest.keychain_leftover = None;
        manifest.last_opened_by = version.to_string();
        if let Some(name) = name {
            let name = name.trim();
            if name.is_empty() {
                bail!("a profile needs a name");
            }
            manifest.name = name.to_string();
        }

        let profiles = self.list()?;
        if let Some(other) = profiles
            .iter()
            .find(|p| p.id() != manifest.id && p.manifest.name.eq_ignore_ascii_case(&manifest.name))
        {
            bail!(
                "a profile named {:?} already exists ({}); give this one another with --name",
                other.name(),
                other.id()
            );
        }
        let dest = self.profiles_dir().join(manifest.id.to_string());
        let existing = profiles.into_iter().find(|p| p.id() == manifest.id);
        match existing {
            Some(old) if !replace => bail!(
                "profile {:?} ({}) is on this machine already; --replace puts the archive's in its place",
                old.name(),
                old.id()
            ),
            Some(old) => {
                let lock = old.lock()?;
                // The key is wherever this machine keeps it; the archive cannot know.
                manifest.keychain = old.manifest.keychain.clone();
                manifest.keychain_leftover = old.manifest.keychain_leftover.clone();
                if manifest.keychain.is_some() || manifest.keychain_leftover.is_some() {
                    manifest.schema = manifest.schema.max(SCHEMA_KEYCHAIN);
                }
                manifest.write(staging)?;
                let aside = self.profiles_dir().join(format!("{REPLACED}{}", manifest.id));
                if aside.exists() {
                    fs::remove_dir_all(&aside).with_context(|| format!("removing {}", aside.display()))?;
                }
                // Windows does not move a folder with an open file in it.
                #[cfg(windows)]
                drop(lock);
                fs::rename(&old.dir, &aside).with_context(|| format!("moving {} aside", old.dir.display()))?;
                if let Err(e) = fs::rename(staging, &dest) {
                    let _ = fs::rename(&aside, &old.dir);
                    return Err(e).with_context(|| format!("moving the imported profile to {}", dest.display()));
                }
                #[cfg(not(windows))]
                drop(lock);
                if let Err(e) = fs::remove_dir_all(&aside) {
                    log::warn!("removing the replaced profile in {}: {e}", aside.display());
                }
            }
            None => {
                if dest.exists() {
                    bail!("{} exists already, and holds no profile", dest.display());
                }
                manifest.write(staging)?;
                fs::rename(staging, &dest).with_context(|| format!("moving the imported profile to {}", dest.display()))?;
            }
        }
        // Whatever is cached under this id is from the profile as it was.
        let cache_dir = self.cache.join(manifest.id.to_string());
        if let Err(e) = fs::remove_dir_all(&cache_dir) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("clearing {}: {e}", cache_dir.display());
            }
        }
        if self.default_id()?.is_none() {
            self.set_default(manifest.id)?;
        }
        Ok(Profile {
            dir: dest,
            cache_dir,
            manifest,
        })
    }

    /// Finish what an import that stopped partway left: a staging folder goes, and a
    /// replaced profile goes too once its successor is in place, or moves back when it is
    /// not. A profile [`Roots::remove`] moved aside goes as well. Call with the roots locked. Only logs, so a stray folder never keeps Beacon
    /// from starting.
    pub(crate) fn recover_interrupted_import(&self) {
        let Ok(entries) = fs::read_dir(self.profiles_dir()) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let dir = entry.path();
            if name.starts_with(STAGING) || name.starts_with(REMOVING) {
                if let Err(e) = fs::remove_dir_all(&dir) {
                    log::warn!("removing {}: {e}", dir.display());
                }
            } else if let Some(id) = name.strip_prefix(REPLACED) {
                let home = self.profiles_dir().join(id);
                let result = if home.join(MANIFEST).exists() {
                    fs::remove_dir_all(&dir)
                } else {
                    log::warn!("an import stopped before its profile was in place; moving {} back", dir.display());
                    fs::rename(&dir, &home)
                };
                if let Err(e) = result {
                    log::warn!("tidying {}: {e}", dir.display());
                }
            }
        }
    }
}

/// Read the archive into `staging`: `beacon-export.toml` first, then exactly the files it
/// lists, each checked. Returns the list and the archive's `profile.toml`, which is not
/// written, since it decides where the profile goes.
fn unpack(archive: impl Read, staging: &Path) -> Result<(Contents, Vec<u8>)> {
    let mut archive = tar::Archive::new(GzDecoder::new(archive));
    let mut entries = archive.entries().context("reading the archive")?;

    let mut first = entries
        .next()
        .ok_or_else(|| anyhow!("the archive is empty"))?
        .context("reading the archive")?;
    if first.path_bytes().as_ref() != EXPORT_MANIFEST.as_bytes() || first.header().entry_type() != tar::EntryType::Regular {
        bail!("this is not a Beacon profile export: it does not start with {EXPORT_MANIFEST}");
    }
    let text = read_capped(&mut first, MAX_LIST_BYTES, EXPORT_MANIFEST)?;
    let text = String::from_utf8(text).with_context(|| format!("{EXPORT_MANIFEST} is not text"))?;
    // The format alone first: a newer archive may not parse as this one.
    #[derive(Deserialize)]
    struct Format {
        format: u32,
    }
    let format: Format = toml::from_str(&text).with_context(|| format!("parsing {EXPORT_MANIFEST}"))?;
    if format.format > EXPORT_FORMAT {
        bail!(
            "the archive was written by a newer Beacon (export format {}); this build reads format {EXPORT_FORMAT}",
            format.format
        );
    }
    let contents: Contents = toml::from_str(&text).with_context(|| format!("parsing {EXPORT_MANIFEST}"))?;

    if contents.files.len() > MAX_FILES {
        bail!("the archive lists {} files, more than a profile has", contents.files.len());
    }
    let mut expected = std::collections::HashMap::new();
    let mut total = 0u64;
    for item in &contents.files {
        if !is_profile_file(&item.path) {
            bail!("the archive lists {:?}, which is no file of a profile", item.path);
        }
        total = total.saturating_add(item.size);
        if expected.insert(item.path.as_str(), item).is_some() {
            bail!("the archive lists {:?} twice", item.path);
        }
    }
    if total > MAX_IMPORT_BYTES {
        bail!("the archive holds {total} bytes, more than an import takes");
    }
    if !expected.contains_key(MANIFEST) {
        bail!("the archive holds no {MANIFEST}");
    }

    let mut manifest = None;
    for entry in entries {
        let mut entry = entry.context("reading the archive")?;
        let raw = entry.path_bytes().into_owned();
        let path = std::str::from_utf8(&raw).map_err(|_| anyhow!("the archive holds a path that is not text"))?;
        let entry_type = entry.header().entry_type();
        // An archive packed again by hand may name the folders; nothing else but files.
        let folders = [PREFIX.to_string(), format!("{PREFIX}/{LOCAL_STORAGE_DIR}")];
        if entry_type == tar::EntryType::Directory && folders.iter().any(|f| f == path.trim_end_matches('/')) {
            continue;
        }
        let Some(rel) = path.strip_prefix(PREFIX).and_then(|p| p.strip_prefix('/')) else {
            bail!("the archive holds {path:?}, outside {PREFIX}/");
        };
        if entry_type != tar::EntryType::Regular {
            bail!("the archive holds {path:?} as something other than a plain file");
        }
        let Some(item) = expected.remove(rel) else {
            bail!("the archive holds {path:?}, which its list does not name, or names once");
        };
        if entry.size() != item.size {
            bail!("{path:?} in the archive is not the size its list gives");
        }
        if rel == MANIFEST {
            let bytes = read_capped(&mut entry, MAX_MANIFEST_BYTES, MANIFEST)?;
            if hex(&Sha256::digest(&bytes)) != item.sha256 {
                bail!("{path:?} in the archive does not match its list");
            }
            manifest = Some(bytes);
            continue;
        }
        let dest = staging.join(rel);
        if let Some(parent) = dest.parent() {
            if parent != staging {
                make_private_dir(parent)?;
            }
        }
        let mut out = private_file(&dest)?;
        let mut reader = Hashing::new((&mut entry).take(item.size));
        std::io::copy(&mut reader, &mut out).with_context(|| format!("unpacking {path:?}"))?;
        if reader.len != item.size || reader.digest() != item.sha256 {
            bail!("{path:?} in the archive does not match its list");
        }
        out.sync_all()?;
    }
    if let Some(missing) = expected.keys().next() {
        bail!("the archive lists {missing:?} but does not hold it");
    }
    let manifest = manifest.ok_or_else(|| anyhow!("the archive holds no {MANIFEST}"))?;
    Ok((contents, manifest))
}

/// Whether `rel` is a path an export writes under `profile/`.
fn is_profile_file(rel: &str) -> bool {
    if rel == MANIFEST || SEALED_FILES.contains(&rel) {
        return true;
    }
    if Store::ALL
        .iter()
        .any(|s| rel == s.file_name() || rel.strip_suffix("-wal") == Some(s.file_name()))
    {
        return true;
    }
    rel.strip_prefix(LOCAL_STORAGE_DIR)
        .and_then(|r| r.strip_prefix('/'))
        .is_some_and(is_area_name)
}

/// The engine names a localStorage area by the hex sha256 of what it holds, plus `.json`.
fn is_area_name(name: &str) -> bool {
    name.strip_suffix(".json")
        .is_some_and(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Whether `path` is a plain file; `false` when there is nothing there, an error for a link
/// or anything else, which an export does not follow.
fn regular_file(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() => Ok(true),
        Ok(_) => bail!("{} is not a plain file", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Whether `path` starts with SQLite's own header, so is a database nothing encrypted.
fn is_plaintext_database(path: &Path) -> bool {
    let mut header = [0u8; 16];
    File::open(path).and_then(|mut f| f.read_exact(&mut header)).is_ok() && &header == b"SQLite format 3\0"
}

/// A new file only the owner can read; an error when it exists.
fn private_file(path: &Path) -> Result<File> {
    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).with_context(|| format!("creating {}", path.display()))
}

fn read_capped(reader: impl Read, cap: u64, what: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(cap + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > cap {
        bail!("{what} in the archive is larger than one can be");
    }
    Ok(bytes)
}

fn header(size: u64, mtime: u64) -> tar::Header {
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Regular);
    header.set_size(size);
    header.set_mode(0o600);
    header.set_mtime(mtime);
    header
}

fn append(tar: &mut tar::Builder<impl Write>, path: &str, data: &[u8], mtime: u64) -> Result<()> {
    let mut header = header(data.len() as u64, mtime);
    tar.append_data(&mut header, path, data)
        .with_context(|| format!("archiving {path}"))
}

fn hash_file(path: &Path) -> Result<(u64, String)> {
    let file = File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut reader = Hashing::new(file);
    std::io::copy(&mut reader, &mut std::io::sink()).with_context(|| format!("reading {}", path.display()))?;
    Ok((reader.len, reader.digest()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// A reader that hashes and counts what passes through it.
struct Hashing<R> {
    inner: R,
    hasher: Sha256,
    len: u64,
}

impl<R: Read> Hashing<R> {
    fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            len: 0,
        }
    }

    fn digest(&self) -> String {
        hex(&self.hasher.clone().finalize())
    }
}

impl<R: Read> Read for Hashing<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.hasher.update(&buf[..n]);
        self.len += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crypto::MemoryKeyStore;

    const V: &str = "0.1.0";
    const AREA: &str = "local-storage/00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff.json";

    fn roots(tmp: &tempfile::TempDir, name: &str) -> Roots {
        Roots {
            data: tmp.path().join(name).join("data"),
            cache: tmp.path().join(name).join("cache"),
        }
    }

    /// A profile as Beacon leaves it: keyed, stores encrypted, small files sealed.
    fn filled(roots: &Roots, name: &str, keys: &MemoryKeyStore) -> (Profile, ProfileKey) {
        let mut p = roots.create(name, V).unwrap();
        let key = p.unlock(Some(keys)).unwrap();
        fs::write(p.dir.join("cookies.db"), [0x5a; 4096]).unwrap();
        fs::write(p.dir.join("places.db"), [0xa5; 8192]).unwrap();
        fs::write(p.dir.join("places.db-wal"), [0x11; 100]).unwrap();
        fs::write(p.dir.join("places.db-shm"), [0x22; 100]).unwrap();
        fs::write(p.dir.join("fetcher-cookies.db"), "stale").unwrap();
        p.write_sealed(&key, "session.tabs", b"https://example.com\n").unwrap();
        fs::create_dir_all(p.dir.join(LOCAL_STORAGE_DIR)).unwrap();
        fs::write(p.dir.join(AREA), b"{}").unwrap();
        fs::write(p.dir.join("local-storage/stray.tmp"), b"half").unwrap();
        (p, key)
    }

    fn export(p: &Profile, wrap: Option<(&ProfileKey, &str)>) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        p.export(&mut out, wrap, V)?;
        Ok(out)
    }

    #[test]
    fn a_profile_travels_and_opens_with_the_export_passphrase() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (roots(&tmp, "here"), roots(&tmp, "there"));
        let keys = MemoryKeyStore::default();
        let (p, key) = filled(&here, "Work", &keys);
        let archive = export(&p, Some((&key, "carry me"))).unwrap();
        // The wrap went into the archive only.
        assert!(!Profile::at(&p.dir, &here.cache).unwrap().has_passphrase());

        let mut q = there.import(archive.as_slice(), None, false, "0.2.0").unwrap();
        assert_eq!((q.id(), q.name(), q.zone_id()), (p.id(), "Work", p.zone_id()));
        assert_eq!(q.dir, there.profiles_dir().join(p.id().to_string()));
        assert_eq!(there.default_id().unwrap(), Some(p.id()));
        assert_eq!(q.manifest.last_opened_by, "0.2.0");
        for name in ["cookies.db", "places.db", "places.db-wal", "session.tabs", AREA] {
            assert_eq!(fs::read(q.dir.join(name)).unwrap(), fs::read(p.dir.join(name)).unwrap(), "{name}");
        }
        for name in ["places.db-shm", "fetcher-cookies.db", "lock", "local-storage/stray.tmp"] {
            assert!(!q.dir.join(name).exists(), "{name}");
        }
        // Locked until the passphrase files its key here, and then the same key.
        let elsewhere = MemoryKeyStore::default();
        assert!(q.key_from(Some(&elsewhere)).unwrap_err().downcast_ref::<Locked>().is_some());
        assert!(q.unlock_with_passphrase(&elsewhere, "wrong").is_err());
        let back = q.unlock_with_passphrase(&elsewhere, "carry me").unwrap();
        assert_eq!(back.check(), key.check());
        assert_eq!(q.read_sealed(&back, "session.tabs").unwrap().unwrap(), b"https://example.com\n");
        assert!(q.unlock(Some(&elsewhere)).is_ok());
        // Nothing left over beside it.
        let left: Vec<_> = fs::read_dir(there.profiles_dir()).unwrap().collect();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn a_profile_with_a_passphrase_carries_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (roots(&tmp, "here"), roots(&tmp, "there"));
        let keys = MemoryKeyStore::default();
        let (mut p, key) = filled(&here, "Home", &keys);
        p.set_passphrase(&key, "own", crypto::TEST_KDF).unwrap();
        p.manifest.keychain = Some("private".into());
        p.manifest.schema = SCHEMA_KEYCHAIN;
        p.manifest.write(&p.dir).unwrap();
        let q = there.import(export(&p, None).unwrap().as_slice(), None, false, V).unwrap();
        assert_eq!(q.manifest.keychain, None);
        assert_eq!(q.manifest.schema, SCHEMA_CRYPTO);
        assert_eq!(q.key_from_passphrase("own").unwrap().check(), key.check());
    }

    #[test]
    fn only_a_fully_encrypted_profile_is_exported() {
        let tmp = tempfile::tempdir().unwrap();
        let here = roots(&tmp, "here");
        let keys = MemoryKeyStore::default();
        let fresh = here.create("Fresh", V).unwrap();
        let err = export(&fresh, None).unwrap_err();
        assert!(err.to_string().contains("not encrypted yet"), "{err}");

        let (p, key) = filled(&here, "Work", &keys);
        let err = export(&p, None).unwrap_err();
        assert!(err.to_string().contains("no passphrase"), "{err}");
        assert!(export(&p, Some((&key, ""))).is_err());
        let other = ProfileKey::generate(p.id());
        assert!(export(&p, Some((&other, "pw"))).is_err());

        let mut plain = b"SQLite format 3\0".to_vec();
        plain.resize(4096, 0);
        fs::write(p.dir.join("settings.db"), &plain).unwrap();
        let err = export(&p, Some((&key, "pw"))).unwrap_err();
        assert!(err.to_string().contains("settings.db"), "{err}");
        fs::remove_file(p.dir.join("settings.db")).unwrap();

        fs::write(p.dir.join("picker-clock.txt"), "12:00").unwrap();
        let err = export(&p, Some((&key, "pw"))).unwrap_err();
        assert!(err.to_string().contains("picker-clock.txt"), "{err}");
    }

    #[test]
    fn a_profile_already_here_is_replaced_only_when_asked() {
        let tmp = tempfile::tempdir().unwrap();
        let here = roots(&tmp, "here");
        let keys = MemoryKeyStore::default();
        let (mut p, key) = filled(&here, "Work", &keys);
        let archive = export(&p, Some((&key, "pw"))).unwrap();
        // Since the export: a new file, and the key moved to a collection of its own.
        fs::write(p.dir.join("settings.db"), [0x77; 4096]).unwrap();
        p.manifest.keychain = Some("private".into());
        p.manifest.schema = SCHEMA_KEYCHAIN;
        p.manifest.write(&p.dir).unwrap();

        let err = here.import(archive.as_slice(), None, false, V).unwrap_err();
        assert!(err.to_string().contains("--replace"), "{err}");
        assert!(p.dir.join("settings.db").exists());

        {
            let _open = p.lock().unwrap();
            let err = here.import(archive.as_slice(), None, true, V).unwrap_err();
            assert!(err.to_string().contains("in use"), "{err}");
        }
        fs::create_dir_all(&p.cache_dir).unwrap();
        let q = here.import(archive.as_slice(), None, true, V).unwrap();
        assert_eq!(q.dir, p.dir);
        assert!(!q.dir.join("settings.db").exists());
        assert!(!p.cache_dir.exists());
        assert_eq!(q.manifest.keychain.as_deref(), Some("private"));
        assert_eq!(q.manifest.schema, SCHEMA_KEYCHAIN);
        assert_eq!(here.list().unwrap().len(), 1);
    }

    #[test]
    fn a_name_in_use_needs_another() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (roots(&tmp, "here"), roots(&tmp, "there"));
        let keys = MemoryKeyStore::default();
        let (p, key) = filled(&here, "Work", &keys);
        let archive = export(&p, Some((&key, "pw"))).unwrap();
        let theirs = there.create("work", V).unwrap();
        let err = there.import(archive.as_slice(), None, false, V).unwrap_err();
        assert!(err.to_string().contains("--name"), "{err}");
        let q = there.import(archive.as_slice(), Some("Work from home"), false, V).unwrap();
        assert_eq!(q.name(), "Work from home");
        // The first profile there stays the default.
        assert_eq!(there.default_id().unwrap(), Some(theirs.id()));
    }

    /// An archive put together by hand: `beacon-export.toml` listing `listed`, then the
    /// entries as given, paths written raw so `..` and the like get in.
    fn forged(listed: &[(&str, &[u8])], entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        forged_for(Uuid::new_v4(), listed, entries)
    }

    fn forged_for(id: Uuid, listed: &[(&str, &[u8])], entries: &[(&str, &[u8], tar::EntryType)]) -> Vec<u8> {
        let contents = Contents {
            format: EXPORT_FORMAT,
            created_by: V.into(),
            exported_at: 0,
            id,
            name: "Forged".into(),
            files: listed
                .iter()
                .map(|(path, data)| Listed {
                    path: path.to_string(),
                    size: data.len() as u64,
                    sha256: hex(&Sha256::digest(data)),
                })
                .collect(),
        };
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        append(&mut tar, EXPORT_MANIFEST, toml::to_string(&contents).unwrap().as_bytes(), 0).unwrap();
        for (path, data, kind) in entries {
            let mut h = header(data.len() as u64, 0);
            h.set_entry_type(*kind);
            let name = &mut h.as_old_mut().name;
            name[..path.len()].copy_from_slice(path.as_bytes());
            if *kind == tar::EntryType::Symlink {
                h.set_link_name("/etc/passwd").unwrap();
            }
            h.set_cksum();
            tar.append(&h, *data).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn a_crafted_archive_is_refused_and_leaves_nothing() {
        use tar::EntryType::{Regular, Symlink};
        let tmp = tempfile::tempdir().unwrap();
        let there = roots(&tmp, "there");
        let m: &[u8] = b"schema = 1";
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("escape", forged(&[("profile.toml", m)], &[("profile/../../evil", b"x", Regular)])),
            ("unlisted", forged(&[("profile.toml", m)], &[("profile/cookies.db", b"x", Regular)])),
            (
                "not a profile file",
                forged(&[("evil.sh", b"x")], &[("profile/evil.sh", b"x", Regular)]),
            ),
            (
                "link",
                forged(&[("profile.toml", m), ("cookies.db", b"")], &[("profile/cookies.db", b"", Symlink)]),
            ),
            (
                "hash",
                forged(
                    &[("profile.toml", m), ("cookies.db", b"good")],
                    &[("profile/cookies.db", b"evil", Regular)],
                ),
            ),
            ("missing", forged(&[("profile.toml", m)], &[])),
            (
                "area name",
                forged(
                    &[("local-storage/x.json", b"x")],
                    &[("profile/local-storage/x.json", b"x", Regular)],
                ),
            ),
        ];
        for (what, archive) in cases {
            assert!(there.import(archive.as_slice(), None, false, V).is_err(), "{what}");
            assert!(!tmp.path().join("evil").exists(), "{what}");
            let left = fs::read_dir(there.profiles_dir()).unwrap().count();
            assert_eq!(left, 0, "{what}");
        }

        // Not ours at all, or not first.
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        append(&mut tar, "profile/profile.toml", m, 0).unwrap();
        let archive = tar.into_inner().unwrap().finish().unwrap();
        let err = there.import(archive.as_slice(), None, false, V).unwrap_err();
        assert!(err.to_string().contains("not a Beacon profile export"), "{err}");
        assert!(there.import(&b"plain text"[..], None, false, V).is_err());

        // A newer format is refused before anything in it is looked at.
        let mut tar = tar::Builder::new(GzEncoder::new(Vec::new(), flate2::Compression::fast()));
        append(&mut tar, EXPORT_MANIFEST, b"format = 99\nwhatever = { new = true }", 0).unwrap();
        let archive = tar.into_inner().unwrap().finish().unwrap();
        let err = there.import(archive.as_slice(), None, false, V).unwrap_err();
        assert!(err.to_string().contains("newer Beacon"), "{err}");
    }

    #[test]
    fn an_archive_without_a_wrapped_key_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (roots(&tmp, "here"), roots(&tmp, "there"));
        let keys = MemoryKeyStore::default();
        let (p, _) = filled(&here, "Work", &keys);
        let manifest = toml::to_string_pretty(&p.manifest).unwrap();
        let m = manifest.as_bytes();
        let archive = forged_for(p.id(), &[(MANIFEST, m)], &[("profile/profile.toml", m, tar::EntryType::Regular)]);
        let err = there.import(archive.as_slice(), None, false, V).unwrap_err();
        assert!(err.to_string().contains("passphrase-wrapped key"), "{err}");
        assert_eq!(fs::read_dir(there.profiles_dir()).unwrap().count(), 0);
    }

    #[test]
    fn a_damaged_wrapped_key_is_refused_before_it_replaces_anything() {
        let tmp = tempfile::tempdir().unwrap();
        let (here, there) = (roots(&tmp, "here"), roots(&tmp, "there"));
        let keys = MemoryKeyStore::default();
        let (mut p, key) = filled(&here, "Work", &keys);
        p.set_passphrase(&key, "pw", crypto::TEST_KDF).unwrap();
        there.import(export(&p, None).unwrap().as_slice(), None, false, V).unwrap();

        type Damage = fn(&mut Crypto);
        let damages: [(&str, Damage); 7] = [
            ("kdf", |c| c.passphrase.as_mut().unwrap().kdf = "scrypt".into()),
            ("cost", |c| c.passphrase.as_mut().unwrap().memory_kib = u32::MAX),
            ("salt", |c| c.passphrase.as_mut().unwrap().salt = "00".into()),
            ("nonce", |c| c.passphrase.as_mut().unwrap().nonce.truncate(46)),
            ("wrapped", |c| c.passphrase.as_mut().unwrap().wrapped.truncate(94)),
            ("not hex", |c| c.passphrase.as_mut().unwrap().wrapped.replace_range(..2, "zz")),
            ("check", |c| c.check.truncate(30)),
        ];
        for (what, damage) in damages {
            let mut manifest = p.manifest.clone();
            damage(manifest.crypto.as_mut().unwrap());
            let manifest = toml::to_string_pretty(&manifest).unwrap();
            let m = manifest.as_bytes();
            let archive = forged_for(p.id(), &[(MANIFEST, m)], &[("profile/profile.toml", m, tar::EntryType::Regular)]);
            let err = there.import(archive.as_slice(), None, true, V).unwrap_err();
            assert!(err.to_string().contains("damaged"), "{what}: {err:#}");
            // The profile it would have replaced is untouched, and still opens.
            let q = there.find("Work").unwrap().unwrap();
            assert_eq!(
                fs::read(q.dir.join("cookies.db")).unwrap(),
                fs::read(p.dir.join("cookies.db")).unwrap()
            );
            q.key_from_passphrase("pw").unwrap();
            assert_eq!(fs::read_dir(there.profiles_dir()).unwrap().count(), 1, "{what}");
        }
    }

    #[test]
    fn an_import_cut_short_is_tidied_on_the_next_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let here = roots(&tmp, "here");
        let p = here.create("Work", V).unwrap();
        let aside = here.profiles_dir().join(format!("{REPLACED}{}", p.id()));
        fs::rename(&p.dir, &aside).unwrap();
        fs::create_dir_all(here.profiles_dir().join(format!("{STAGING}x"))).unwrap();
        // Skipped while it waits, never listed twice.
        assert!(here.list().unwrap().is_empty());
        let again = here.open(&Selection::Named("Work".into()), V).unwrap();
        assert_eq!(again.id(), p.id());
        assert_eq!(fs::read_dir(here.profiles_dir()).unwrap().count(), 1);
    }
}
