//! The profile key and what it unlocks.
//!
//! Every profile on a machine with a keychain gets a random 256-bit key, kept in the OS
//! keychain under the profile's id. The databases are encrypted with keys derived from it,
//! one per store, so no two files share a key. On its own that key never leaves the machine:
//! copy the folder elsewhere and nothing opens it.
//!
//! A passphrase makes the profile portable. It wraps the profile key (argon2id, then
//! XChaCha20-Poly1305) and the wrapped key goes into `profile.toml`, so it travels with the
//! folder. On a machine whose keychain does not hold the key yet, the passphrase unwraps it
//! once and the keychain keeps it from then on.
//!
//! Changing or removing the passphrase moves the profile to a new key and re-encrypts the
//! databases with it (`Profile::rotate_key`): re-wrapping the same key would leave every
//! earlier copy of `profile.toml` able to open them with the passphrase it carried. While a
//! rotation is under way the new key waits in the keychain's [`Slot::Next`].
//!
//! ```toml
//! [crypto]
//! version = 1
//! check = "<hex>"            # derived from the key: tells a stale keychain entry apart
//!
//! [crypto.passphrase]        # only when a passphrase is set
//! kdf = "argon2id"
//! normalization = "nfc"      # how the passphrase is prepared; absent: the bytes as typed
//! memory_kib = 65536
//! iterations = 3
//! parallelism = 1
//! salt = "<hex>"
//! nonce = "<hex>"
//! wrapped = "<hex>"
//! ```

use anyhow::{anyhow, bail, Context, Result};
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit, OsRng, Payload};
use chacha20poly1305::XChaCha20Poly1305;
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::fs;
use std::path::{Path, PathBuf};
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zeroize::Zeroizing;

/// How store keys are derived from the profile key. Bumped if that ever changes, since an
/// older derivation would silently produce keys that open nothing.
pub const CRYPTO_VERSION: u32 = 1;

/// The keychain service every profile key is filed under; the account is `profile-<id>`.
pub const KEYCHAIN_SERVICE: &str = "gosub-beacon";

/// The first bytes of a file [`ProfileKey::seal`] wrote.
pub const SEALED_MAGIC: &[u8; 8] = b"GSBSEAL1";

/// Whether `data` is a sealed file, as opposed to one written before Beacon sealed them.
pub fn is_sealed(data: &[u8]) -> bool {
    data.starts_with(SEALED_MAGIC)
}

/// `[crypto]` in `profile.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crypto {
    pub version: u32,
    /// A value derived from the profile key, so a key from the keychain can be checked
    /// against this profile before it is used on any database.
    pub check: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<Wrapped>,
}

/// `[crypto.passphrase]`: the profile key, wrapped by a passphrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wrapped {
    pub kdf: String,
    /// How the passphrase was prepared before the key derivation. A manifest from before
    /// Beacon normalised passphrases has none, and its passphrase is the bytes as typed.
    #[serde(default)]
    pub normalization: Normalization,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
    pub nonce: String,
    pub wrapped: String,
}

/// A [`Wrapped`] read back: what [`ProfileKey::unwrap`] needs, each part checked.
struct WrappedParts {
    params: KdfParams,
    salt: Vec<u8>,
    nonce: Vec<u8>,
    wrapped: Vec<u8>,
}

impl Wrapped {
    /// Whether a passphrase could unwrap this at all: a derivation and cost Beacon accepts,
    /// and a salt, nonce and wrapped key of the right sizes. Says nothing about the
    /// passphrase; a manifest from elsewhere is checked with it before it is trusted.
    pub(crate) fn validate(&self) -> Result<()> {
        self.parts().map(drop)
    }

    fn parts(&self) -> Result<WrappedParts> {
        if self.kdf != "argon2id" {
            bail!("unknown key derivation {:?}", self.kdf);
        }
        let params = KdfParams {
            memory_kib: self.memory_kib,
            iterations: self.iterations,
            parallelism: self.parallelism,
        };
        params.argon2()?;
        let salt = unhex(&self.salt).context("the wrapped key's salt")?;
        if salt.len() < argon2::MIN_SALT_LEN {
            bail!(
                "the wrapped key's salt is {} bytes, fewer than {}",
                salt.len(),
                argon2::MIN_SALT_LEN
            );
        }
        let nonce = unhex(&self.nonce).context("the wrapped key's nonce")?;
        if nonce.len() != 24 {
            bail!("the wrapped key's nonce is {} bytes, not 24", nonce.len());
        }
        let wrapped = unhex(&self.wrapped).context("the wrapped key")?;
        // A 32-byte key and its 16-byte tag.
        if wrapped.len() != 48 {
            bail!("the wrapped key is {} bytes, not 48", wrapped.len());
        }
        Ok(WrappedParts {
            params,
            salt,
            nonce,
            wrapped,
        })
    }
}

/// What is done to a passphrase before it is hashed. Recorded with the wrap, so a wrap made
/// one way is never read another.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Normalization {
    /// The bytes as typed. What every wrap before `normalization` was recorded used.
    #[default]
    None,
    /// Unicode NFC, so the passphrase hashes the same however the machine it is typed on
    /// composes its characters: a copied profile is meant to open on another one.
    Nfc,
}

/// Argon2id cost for wrapping with a passphrase.
#[derive(Debug, Clone, Copy)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    /// The most a wrapped key may ask for. The cost comes from the manifest, which travels
    /// with a copied profile; without a ceiling a crafted one makes `unlock` claim all the
    /// memory there is, or spin for hours. Four times the default memory, and with
    /// [`Self::MAX_WORK`] no more than sixteen times its work.
    pub const MAX: Self = Self {
        memory_kib: 256 * 1024,
        iterations: 16,
        parallelism: 8,
    };

    /// The most memory times passes a wrapped key may ask for, in KiB: sixteen times the
    /// default's. Each limit in [`Self::MAX`] alone still allows four times this.
    pub const MAX_WORK: u64 = 16 * 64 * 1024 * 3;

    /// These as argon2 takes them, if Beacon accepts them.
    fn argon2(self) -> Result<argon2::Params> {
        let max = Self::MAX;
        let work = u64::from(self.memory_kib) * u64::from(self.iterations);
        if self.memory_kib > max.memory_kib
            || self.iterations > max.iterations
            || self.parallelism > max.parallelism
            || work > Self::MAX_WORK
        {
            bail!(
                "argon2 cost {} KiB x {} passes x {} lanes is beyond what Beacon accepts ({} KiB x {} x {}, {} KiB x passes in all)",
                self.memory_kib,
                self.iterations,
                self.parallelism,
                max.memory_kib,
                max.iterations,
                max.parallelism,
                Self::MAX_WORK
            );
        }
        argon2::Params::new(self.memory_kib, self.iterations, self.parallelism, Some(32)).map_err(|e| anyhow!("argon2 parameters: {e}"))
    }
}

impl Default for KdfParams {
    /// 64 MiB, three passes: well under a second on a laptop, and the cost an attacker pays
    /// per guess against a copied profile.
    fn default() -> Self {
        Self {
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        }
    }
}

/// The stores Beacon keeps in a profile. Each gets its own key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    Settings,
    Cookies,
    Places,
    LocalStorage,
    /// The certificates `gemini://` trusted on first use.
    KnownHosts,
}

impl Store {
    pub const ALL: [Store; 5] = [
        Store::Settings,
        Store::Cookies,
        Store::Places,
        Store::LocalStorage,
        Store::KnownHosts,
    ];

    /// The database file inside the profile folder.
    pub fn file_name(self) -> &'static str {
        match self {
            Store::Settings => "settings.db",
            Store::Cookies => "cookies.db",
            Store::Places => "places.db",
            Store::LocalStorage => "local-storage.db",
            Store::KnownHosts => "known-hosts.db",
        }
    }

    fn info(self) -> &'static [u8] {
        match self {
            Store::Settings => b"gosub-beacon store v1 settings",
            Store::Cookies => b"gosub-beacon store v1 cookies",
            Store::Places => b"gosub-beacon store v1 places",
            Store::LocalStorage => b"gosub-beacon store v1 local-storage",
            Store::KnownHosts => b"gosub-beacon store v1 known-hosts",
        }
    }
}

/// A profile's 256-bit key. Wiped from memory on drop.
#[derive(Clone)]
pub struct ProfileKey {
    bytes: Zeroizing<[u8; 32]>,
    profile: Uuid,
}

impl std::fmt::Debug for ProfileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ProfileKey({}, ..)", self.profile)
    }
}

impl ProfileKey {
    pub(crate) fn generate(profile: Uuid) -> Self {
        let mut bytes = Zeroizing::new([0u8; 32]);
        bytes.copy_from_slice(&XChaCha20Poly1305::generate_key(&mut OsRng));
        Self { bytes, profile }
    }

    fn from_bytes(profile: Uuid, bytes: &[u8]) -> Result<Self> {
        let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("a profile key is 32 bytes"))?;
        Ok(Self {
            bytes: Zeroizing::new(bytes),
            profile,
        })
    }

    /// The key as the keychain files it: hex, so every backend sees text. The Secret
    /// Service stores a secret as a string on some desktops (KWallet), and raw bytes that
    /// are not UTF-8 may not come back as they went in.
    fn filed(&self) -> Zeroizing<String> {
        Zeroizing::new(hex(self.bytes.as_slice()))
    }

    /// A key read back from the keychain: the hex [`filed`](Self::filed) writes, or the 32
    /// raw bytes a build before it filed.
    fn from_filed(profile: Uuid, secret: &[u8]) -> Result<Self> {
        if secret.len() == 64 {
            let text = std::str::from_utf8(secret).map_err(|_| anyhow!("the filed profile key is not hex"))?;
            return Self::from_bytes(profile, &Zeroizing::new(unhex(text)?));
        }
        Self::from_bytes(profile, secret)
    }

    /// The key for one store's database. Salted with the profile id, so two profiles that
    /// somehow share key bytes still have different store keys. That makes the id part of
    /// the key material: it never changes for a folder, or nothing in it would open.
    pub fn store_key(&self, store: Store) -> Zeroizing<[u8; 32]> {
        self.derive(store.info())
    }

    /// Encrypt one of the profile's small files (the open tabs, the pickers' history): a
    /// key derived for `name`, XChaCha20-Poly1305 with a random nonce and the name as
    /// associated data, behind [`SEALED_MAGIC`] so a file from before sealing is told apart.
    pub fn seal(&self, name: &str, plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = self.file_key(name);
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let sealed = XChaCha20Poly1305::new(key.as_slice().into())
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("sealing {name} failed"))?;
        let mut out = Vec::with_capacity(SEALED_MAGIC.len() + nonce.len() + sealed.len());
        out.extend_from_slice(SEALED_MAGIC);
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&sealed);
        Ok(out)
    }

    /// The plaintext of a file [`seal`](Self::seal) wrote under this key and `name`.
    pub fn unseal(&self, name: &str, sealed: &[u8]) -> Result<Vec<u8>> {
        let Some(body) = sealed.strip_prefix(SEALED_MAGIC) else {
            bail!("{name} is not a sealed file");
        };
        if body.len() < 24 {
            bail!("{name} is cut short");
        }
        let (nonce, sealed) = body.split_at(24);
        XChaCha20Poly1305::new(self.file_key(name).as_slice().into())
            .decrypt(
                nonce.into(),
                Payload {
                    msg: sealed,
                    aad: name.as_bytes(),
                },
            )
            .map_err(|_| anyhow!("{name} was not sealed with this profile's key"))
    }

    fn file_key(&self, name: &str) -> Zeroizing<[u8; 32]> {
        let mut info = b"gosub-beacon file v1 ".to_vec();
        info.extend_from_slice(name.as_bytes());
        self.derive(&info)
    }

    fn derive(&self, info: &[u8]) -> Zeroizing<[u8; 32]> {
        let hk = Hkdf::<Sha256>::new(Some(self.profile.as_bytes()), self.bytes.as_slice());
        let mut out = Zeroizing::new([0u8; 32]);
        // Only fails for an output longer than 255 hash lengths.
        hk.expand(info, out.as_mut_slice()).expect("32 bytes is a valid HKDF output length");
        out
    }

    pub(crate) fn check(&self) -> String {
        hex(&self.derive(b"gosub-beacon key check v1")[..16])
    }

    /// `[crypto]` for this key, without a passphrase.
    pub(crate) fn manifest_entry(&self) -> Crypto {
        Crypto {
            version: CRYPTO_VERSION,
            check: self.check(),
            passphrase: None,
        }
    }

    /// Wrap this key with `passphrase` for `profile.toml`.
    pub(crate) fn wrap(&self, passphrase: &str, params: KdfParams) -> Result<Wrapped> {
        self.wrap_as(passphrase, params, Normalization::Nfc)
    }

    fn wrap_as(&self, passphrase: &str, params: KdfParams, normalization: Normalization) -> Result<Wrapped> {
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&XChaCha20Poly1305::generate_nonce(&mut OsRng)[..16]);
        let kek = kek(passphrase, normalization, &salt, params)?;
        let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
        let wrapped = XChaCha20Poly1305::new(kek.as_slice().into())
            .encrypt(
                &nonce,
                Payload {
                    msg: self.bytes.as_slice(),
                    aad: &aad(self.profile),
                },
            )
            .map_err(|_| anyhow!("wrapping the profile key failed"))?;
        Ok(Wrapped {
            kdf: "argon2id".into(),
            normalization,
            memory_kib: params.memory_kib,
            iterations: params.iterations,
            parallelism: params.parallelism,
            salt: hex(&salt),
            nonce: hex(&nonce),
            wrapped: hex(&wrapped),
        })
    }

    /// The key `wrapped` holds, if `passphrase` is the one it was wrapped with.
    pub(crate) fn unwrap(profile: Uuid, wrapped: &Wrapped, passphrase: &str) -> Result<Self> {
        let parts = wrapped.parts()?;
        let kek = kek(passphrase, wrapped.normalization, &parts.salt, parts.params)?;
        let bytes = Zeroizing::new(
            XChaCha20Poly1305::new(kek.as_slice().into())
                .decrypt(
                    parts.nonce.as_slice().into(),
                    Payload {
                        msg: &parts.wrapped,
                        aad: &aad(profile),
                    },
                )
                .map_err(|_| anyhow!("wrong passphrase"))?,
        );
        Self::from_bytes(profile, &bytes)
    }
}

/// Binds a wrapped key to its profile, so a `[crypto.passphrase]` pasted into another
/// profile's manifest does not unwrap.
fn aad(profile: Uuid) -> Vec<u8> {
    let mut aad = b"gosub-beacon profile key v1 ".to_vec();
    aad.extend_from_slice(profile.as_bytes());
    aad
}

fn kek(passphrase: &str, normalization: Normalization, salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let params = params.argon2()?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    let passphrase: Zeroizing<String> = Zeroizing::new(match normalization {
        Normalization::None => passphrase.to_string(),
        Normalization::Nfc => passphrase.nfc().collect(),
    });
    let mut out = Zeroizing::new([0u8; 32]);
    argon
        .hash_password_into(passphrase.as_bytes(), salt, out.as_mut_slice())
        .map_err(|e| anyhow!("argon2: {e}"))?;
    Ok(out)
}

/// Which of a profile's keys the keychain is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    /// The key the profile is on.
    Current,
    /// The key a rotation in progress is moving the profile to. Empty at any other time;
    /// while it is filled, the databases may be on either key and nothing opens the
    /// profile until the rotation is finished.
    Next,
}

/// Where profile keys are kept on this machine.
pub trait KeyStore {
    /// The key filed for `profile` in `slot`, `Ok(None)` when there is none.
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>>;
    fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()>;
    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()>;
}

/// The OS keychain: Keychain on macOS, Credential Manager on Windows, the Secret Service
/// (GNOME Keyring, KWallet) on Linux.
pub struct OsKeychain {
    // The macOS keychain is reached through `mac`, which has no collections.
    #[cfg_attr(not(any(target_os = "linux", target_os = "windows")), allow(dead_code))]
    collection: Option<String>,
}

impl OsKeychain {
    /// The keychain, or `None` on a platform that has none Beacon can use: Android, where
    /// the key goes into a [`FileKeyStore`] in the app's private storage instead.
    pub fn new() -> Option<Self> {
        cfg!(any(target_os = "linux", target_os = "macos", target_os = "windows")).then_some(Self { collection: None })
    }

    /// Keep keys in the collection labelled `collection` rather than the default one (on
    /// Linux the login keyring, unlocked at login). The Secret Service creates it on the
    /// first key filed there, asking for its password, and asks for that password again
    /// whenever a key is read while it is locked. Linux only: the other keychains ignore
    /// it, and `beaconctl` refuses to set one there.
    pub fn in_collection(mut self, collection: Option<&str>) -> Self {
        self.collection = collection.map(str::to_string);
        self
    }

    /// Whether this platform's keychain has collections [`OsKeychain::in_collection`] can
    /// pick.
    pub fn has_collections() -> bool {
        cfg!(target_os = "linux")
    }

    /// Keep the keychain from asking the user anything. On macOS a keychain item trusts the
    /// application that made it, and asks on screen for any other; with no screen (over
    /// ssh) that call never returns. After this it fails instead, with an error saying to
    /// run once on the Mac itself. Nothing elsewhere: the other keychains do not ask.
    pub fn forbid_dialogs() {
        #[cfg(target_os = "macos")]
        mac::MacKeychain::forbid_dialogs();
    }
}

#[cfg(target_os = "macos")]
mod mac;

#[cfg(target_os = "macos")]
impl KeyStore for OsKeychain {
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
        mac::MacKeychain::new().get(profile, slot)
    }

    fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()> {
        mac::MacKeychain::new().set(key, slot)
    }

    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()> {
        mac::MacKeychain::new().delete(profile, slot)
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl OsKeychain {
    fn entry(&self, profile: Uuid, slot: Slot) -> Result<keyring::Entry> {
        let account = match slot {
            Slot::Current => format!("profile-{profile}"),
            Slot::Next => format!("profile-{profile}.next"),
        };
        // On the Secret Service a target names the collection, and is part of what an entry
        // is found by: a key filed in one collection is not found through another, and
        // moving it is `Profile::move_key`. On Windows a target is the credential's own
        // name instead, so it never gets one.
        let entry = match self.collection.as_deref() {
            Some(collection) if cfg!(target_os = "linux") => keyring::Entry::new_with_target(collection, KEYCHAIN_SERVICE, &account),
            _ => keyring::Entry::new(KEYCHAIN_SERVICE, &account),
        };
        entry.context("opening the keychain")
    }
}

/// How long a keychain call runs before [`patiently`] says it is still waiting.
#[cfg(any(target_os = "linux", target_os = "windows"))]
const KEYCHAIN_PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// Run one keychain call, and say on stderr when it is slow. Not cut short: a locked
/// collection waits on its password prompt for as long as the user takes. But a Secret
/// Service that D-Bus can start and that never answers holds the call until the bus gives
/// up, two minutes on, and Beacon would sit there without a word. Stderr, not the log: the
/// shells filter this crate's warnings out, and `beaconctl` has no logger.
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn patiently<T>(call: impl FnOnce() -> T) -> T {
    let (done, waiting) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|s| {
        s.spawn(move || {
            if waiting.recv_timeout(KEYCHAIN_PATIENCE) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                // Not `eprintln!`, which panics when stderr is gone, and the scope would
                // carry that panic into the keychain call.
                use std::io::Write;
                let _ = writeln!(
                    std::io::stderr(),
                    "gosub-beacon: still waiting for the keychain after {}s: a locked keyring waits for its password; \
                     a Secret Service that does not start fails after two minutes",
                    KEYCHAIN_PATIENCE.as_secs()
                );
            }
        });
        let out = call();
        drop(done);
        out
    })
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
impl KeyStore for OsKeychain {
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
        let entry = self.entry(profile, slot)?;
        // Read as text, the way `set` writes it: on Windows a password is filed as UTF-16,
        // and the raw blob would not be the hex that went in.
        match patiently(|| entry.get_password()) {
            Ok(text) => {
                let text = Zeroizing::new(text);
                Ok(Some(ProfileKey::from_filed(profile, text.as_bytes())?))
            }
            // Not text: the 32 raw bytes a build before this one filed.
            Err(keyring::Error::BadEncoding(raw)) => {
                let raw = Zeroizing::new(raw);
                Ok(Some(ProfileKey::from_filed(profile, &raw)?))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e).context("reading the profile key from the keychain"),
        }
    }

    fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()> {
        let entry = self.entry(key.profile, slot)?;
        patiently(|| entry.set_password(&key.filed())).context("storing the profile key in the keychain")
    }

    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()> {
        let entry = self.entry(profile, slot)?;
        match patiently(|| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e).context("removing the profile key from the keychain"),
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
impl KeyStore for OsKeychain {
    fn get(&self, _profile: Uuid, _slot: Slot) -> Result<Option<ProfileKey>> {
        Ok(None)
    }

    fn set(&self, _key: &ProfileKey, _slot: Slot) -> Result<()> {
        bail!("this platform has no keychain Beacon can use")
    }

    fn delete(&self, _profile: Uuid, _slot: Slot) -> Result<()> {
        Ok(())
    }
}

/// Keys in files of their own, for a platform without a keychain Beacon can use: Android,
/// where the app's private storage, which no other app reads, stands in for one. The folder
/// is outside every profile, so a copied profile folder still travels without its key.
/// Owner-only files, each written whole and renamed into place.
pub struct FileKeyStore {
    dir: PathBuf,
}

impl FileKeyStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self, profile: Uuid, slot: Slot) -> PathBuf {
        self.dir.join(match slot {
            Slot::Current => profile.to_string(),
            Slot::Next => format!("{profile}.next"),
        })
    }
}

impl KeyStore for FileKeyStore {
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
        let path = self.path(profile, slot);
        match fs::read(&path) {
            Ok(filed) => {
                let filed = Zeroizing::new(filed);
                Ok(Some(ProfileKey::from_filed(profile, &filed)?))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading the profile key from {}", path.display())),
        }
    }

    fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()> {
        fs::create_dir_all(&self.dir).with_context(|| format!("creating {}", self.dir.display()))?;
        let path = self.path(key.profile, slot);
        // Appended, not swapped in for an extension: `<id>` and `<id>.next` must not share
        // a temp file.
        let mut tmp = path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            fs::set_permissions(&self.dir, fs::Permissions::from_mode(0o700))
                .with_context(|| format!("setting the mode of {}", self.dir.display()))?;
            options.mode(0o600);
        }
        {
            use std::io::Write;
            let mut file = options.open(&tmp).with_context(|| format!("writing {}", tmp.display()))?;
            file.write_all(key.filed().as_bytes())
                .and_then(|()| file.sync_all())
                .with_context(|| format!("writing {}", tmp.display()))?;
        }
        fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        // The rename lives in the folder: until that is on disk too, a power cut can leave
        // the profile encrypted and its key file gone.
        #[cfg(unix)]
        fs::File::open(&self.dir)
            .and_then(|dir| dir.sync_all())
            .with_context(|| format!("syncing {}", self.dir.display()))?;
        Ok(())
    }

    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()> {
        let path = self.path(profile, slot);
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
}

/// A keychain in memory, for tests.
#[derive(Default)]
pub struct MemoryKeyStore {
    keys: std::sync::Mutex<std::collections::HashMap<(Uuid, Slot), ProfileKey>>,
    refuse: bool,
}

impl MemoryKeyStore {
    /// One whose every `set` fails, like a machine whose keychain is locked or missing.
    pub fn refusing() -> Self {
        Self {
            refuse: true,
            ..Default::default()
        }
    }
}

impl KeyStore for MemoryKeyStore {
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
        Ok(self.keys.lock().unwrap().get(&(profile, slot)).cloned())
    }

    fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()> {
        if self.refuse {
            bail!("keychain unavailable");
        }
        self.keys.lock().unwrap().insert((key.profile, slot), key.clone());
        Ok(())
    }

    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()> {
        self.keys.lock().unwrap().remove(&(profile, slot));
        Ok(())
    }
}

/// Hex, built in place: no per-byte temporaries, since the input may be the key itself.
pub(crate) fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        s.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
    }
    s
}

fn unhex(s: &str) -> Result<Vec<u8>> {
    // Byte offsets below: anything but ASCII could split a character and panic. The errors
    // never quote `s`: it may be a key with one digit damaged. Digits checked here, since
    // `from_str_radix` also takes a sign: "+f" would read as 0x0f.
    if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("not even-length hex");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| anyhow!("not hex")))
        .collect()
}

#[cfg(test)]
pub(crate) const TEST_KDF: KdfParams = KdfParams {
    memory_kib: 64,
    iterations: 1,
    parallelism: 1,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_keys_differ_per_store_and_per_profile() {
        let a = ProfileKey::generate(Uuid::new_v4());
        let same_bytes_other_profile = ProfileKey::from_bytes(Uuid::new_v4(), a.bytes.as_slice()).unwrap();
        let keys: Vec<_> = Store::ALL.iter().map(|s| *a.store_key(*s)).collect();
        for (i, k) in keys.iter().enumerate() {
            assert!(keys[i + 1..].iter().all(|o| o != k));
        }
        assert_ne!(*a.store_key(Store::Cookies), *same_bytes_other_profile.store_key(Store::Cookies));
        // Deterministic: the same key opens the same database next run.
        assert_eq!(*a.store_key(Store::Places), *a.clone().store_key(Store::Places));
    }

    #[test]
    fn wrap_unwraps_only_with_its_passphrase_and_profile() {
        let id = Uuid::new_v4();
        let key = ProfileKey::generate(id);
        let wrapped = key.wrap("correct horse", TEST_KDF).unwrap();
        let back = ProfileKey::unwrap(id, &wrapped, "correct horse").unwrap();
        assert_eq!(back.check(), key.check());
        let err = ProfileKey::unwrap(id, &wrapped, "wrong").unwrap_err();
        assert!(err.to_string().contains("wrong passphrase"), "{err}");
        assert!(ProfileKey::unwrap(Uuid::new_v4(), &wrapped, "correct horse").is_err());
    }

    #[test]
    fn a_sealed_file_opens_only_under_its_key_and_name() {
        let id = Uuid::new_v4();
        let key = ProfileKey::generate(id);
        let sealed = key.seal("session.tabs", b"*https://example.org/\n").unwrap();
        assert!(is_sealed(&sealed));
        assert!(!sealed.windows(7).any(|w| w == b"example"));
        assert_eq!(key.unseal("session.tabs", &sealed).unwrap(), b"*https://example.org/\n");
        assert!(key.unseal("picker-clock.txt", &sealed).is_err());
        assert!(ProfileKey::generate(id).unseal("session.tabs", &sealed).is_err());
        assert!(key.unseal("session.tabs", b"plain text").is_err());
        assert!(key.unseal("session.tabs", &sealed[..20]).is_err());
        // Two seals of the same bytes differ: the nonce is fresh each time.
        assert_ne!(key.seal("session.tabs", b"x").unwrap(), key.seal("session.tabs", b"x").unwrap());
    }

    #[test]
    fn file_key_store_keeps_keys_in_private_files() {
        let tmp = tempfile::tempdir().unwrap();
        let store = FileKeyStore::new(tmp.path().join("keys"));
        let id = Uuid::new_v4();
        assert!(store.get(id, Slot::Current).unwrap().is_none());
        let key = ProfileKey::generate(id);
        store.set(&key, Slot::Current).unwrap();
        store.set(&ProfileKey::generate(id), Slot::Next).unwrap();
        assert_eq!(*store.get(id, Slot::Current).unwrap().unwrap().bytes, *key.bytes);
        assert_ne!(*store.get(id, Slot::Next).unwrap().unwrap().bytes, *key.bytes);
        let filed = std::fs::read_to_string(tmp.path().join("keys").join(id.to_string())).unwrap();
        assert_eq!(filed, *key.filed());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&tmp.path().join("keys")), 0o700);
            assert_eq!(mode(&tmp.path().join("keys").join(id.to_string())), 0o600);
        }
        store.delete(id, Slot::Next).unwrap();
        store.delete(id, Slot::Next).unwrap();
        assert!(store.get(id, Slot::Next).unwrap().is_none());
        assert!(store.get(id, Slot::Current).unwrap().is_some());
    }

    #[test]
    fn a_passphrase_opens_however_its_characters_are_composed() {
        let id = Uuid::new_v4();
        let key = ProfileKey::generate(id);
        // "café" with a precomposed é, then with e plus a combining acute.
        let wrapped = key.wrap("caf\u{e9}", TEST_KDF).unwrap();
        let back = ProfileKey::unwrap(id, &wrapped, "cafe\u{301}").unwrap();
        assert_eq!(back.check(), key.check());
        assert!(ProfileKey::unwrap(id, &wrapped, "cafe").is_err());
        assert_eq!(wrapped.normalization, Normalization::Nfc);

        // A wrap from before normalisation was recorded takes the bytes as typed, and a
        // manifest without the field reads as one.
        let legacy = key.wrap_as("cafe\u{301}", TEST_KDF, Normalization::None).unwrap();
        let mut toml_text = toml::to_string(&legacy).unwrap();
        assert!(toml_text.contains("normalization = \"none\""));
        toml_text = toml_text.replace("normalization = \"none\"\n", "");
        let read: Wrapped = toml::from_str(&toml_text).unwrap();
        assert_eq!(read, legacy);
        assert_eq!(ProfileKey::unwrap(id, &read, "cafe\u{301}").unwrap().check(), key.check());
        assert!(ProfileKey::unwrap(id, &read, "caf\u{e9}").is_err());
    }

    #[test]
    fn a_wrapped_key_asking_too_much_is_refused_before_hashing() {
        let id = Uuid::new_v4();
        let mut wrapped = ProfileKey::generate(id).wrap("pw", TEST_KDF).unwrap();
        wrapped.memory_kib = u32::MAX;
        let err = ProfileKey::unwrap(id, &wrapped, "pw").unwrap_err();
        assert!(err.to_string().contains("beyond what Beacon accepts"), "{err}");
        // Each part within its limit, the two together not.
        wrapped.memory_kib = KdfParams::MAX.memory_kib;
        wrapped.iterations = KdfParams::MAX.iterations;
        let err = ProfileKey::unwrap(id, &wrapped, "pw").unwrap_err();
        assert!(err.to_string().contains("beyond what Beacon accepts"), "{err}");
    }

    #[test]
    fn a_damaged_filed_key_is_not_quoted_in_the_error() {
        let filed = "0".repeat(63) + "g";
        let err = unhex(&filed).unwrap_err();
        assert!(!err.to_string().contains("000"), "{err}");
    }

    #[test]
    fn a_filed_key_reads_back_from_hex_or_raw_bytes() {
        let id = Uuid::new_v4();
        let key = ProfileKey::generate(id);
        let filed = key.filed();
        assert_eq!(filed.len(), 64);
        assert!(filed.is_ascii());
        assert_eq!(*ProfileKey::from_filed(id, filed.as_bytes()).unwrap().bytes, *key.bytes);
        assert_eq!(*ProfileKey::from_filed(id, key.bytes.as_slice()).unwrap().bytes, *key.bytes);
        assert!(ProfileKey::from_filed(id, &[b'z'; 64]).is_err());
        assert!(ProfileKey::from_filed(id, &[1; 31]).is_err());
    }

    #[test]
    fn hex_round_trips() {
        assert_eq!(unhex(&hex(&[0, 1, 0xab, 0xff])).unwrap(), vec![0, 1, 0xab, 0xff]);
        assert!(unhex("abc").is_err());
        assert!(unhex("zz").is_err());
        assert!(unhex("aéa").is_err());
        assert!(unhex("+f").is_err());
    }
}
