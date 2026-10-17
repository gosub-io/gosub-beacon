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
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;
use zeroize::Zeroizing;

/// How store keys are derived from the profile key. Bumped if that ever changes, since an
/// older derivation would silently produce keys that open nothing.
pub const CRYPTO_VERSION: u32 = 1;

/// The keychain service every profile key is filed under; the account is `profile-<id>`.
pub const KEYCHAIN_SERVICE: &str = "gosub-beacon";

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
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
    pub salt: String,
    pub nonce: String,
    pub wrapped: String,
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
    /// memory there is, or spin for hours. Sixteen times the default memory is plenty.
    pub const MAX: Self = Self {
        memory_kib: 1024 * 1024,
        iterations: 64,
        parallelism: 16,
    };
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
}

impl Store {
    pub const ALL: [Store; 4] = [Store::Settings, Store::Cookies, Store::Places, Store::LocalStorage];

    /// The database file inside the profile folder.
    pub fn file_name(self) -> &'static str {
        match self {
            Store::Settings => "settings.db",
            Store::Cookies => "cookies.db",
            Store::Places => "places.db",
            Store::LocalStorage => "local-storage.db",
        }
    }

    fn info(self) -> &'static [u8] {
        match self {
            Store::Settings => b"gosub-beacon store v1 settings",
            Store::Cookies => b"gosub-beacon store v1 cookies",
            Store::Places => b"gosub-beacon store v1 places",
            Store::LocalStorage => b"gosub-beacon store v1 local-storage",
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

    /// The key for one store's database. Salted with the profile id, so the same key in two
    /// profiles (a copied folder given a new id) still gives different store keys.
    pub fn store_key(&self, store: Store) -> Zeroizing<[u8; 32]> {
        self.derive(store.info())
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
        let mut salt = [0u8; 16];
        salt.copy_from_slice(&XChaCha20Poly1305::generate_nonce(&mut OsRng)[..16]);
        let kek = kek(passphrase, &salt, params)?;
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
        if wrapped.kdf != "argon2id" {
            bail!("unknown key derivation {:?}", wrapped.kdf);
        }
        let params = KdfParams {
            memory_kib: wrapped.memory_kib,
            iterations: wrapped.iterations,
            parallelism: wrapped.parallelism,
        };
        let kek = kek(passphrase, &unhex(&wrapped.salt)?, params)?;
        let nonce = unhex(&wrapped.nonce)?;
        if nonce.len() != 24 {
            bail!("the wrapped key's nonce is {} bytes, not 24", nonce.len());
        }
        let bytes = Zeroizing::new(
            XChaCha20Poly1305::new(kek.as_slice().into())
                .decrypt(
                    nonce.as_slice().into(),
                    Payload {
                        msg: &unhex(&wrapped.wrapped)?,
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

fn kek(passphrase: &str, salt: &[u8], params: KdfParams) -> Result<Zeroizing<[u8; 32]>> {
    let max = KdfParams::MAX;
    if params.memory_kib > max.memory_kib || params.iterations > max.iterations || params.parallelism > max.parallelism {
        bail!(
            "argon2 cost {} KiB x {} passes x {} lanes is beyond what Beacon accepts ({} KiB x {} x {})",
            params.memory_kib,
            params.iterations,
            params.parallelism,
            max.memory_kib,
            max.iterations,
            max.parallelism
        );
    }
    let params = argon2::Params::new(params.memory_kib, params.iterations, params.parallelism, Some(32))
        .map_err(|e| anyhow!("argon2 parameters: {e}"))?;
    let argon = argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
    // NFC, so the passphrase hashes the same however the machine it is typed on composes
    // its characters: a copied profile is meant to open on another one.
    let passphrase: Zeroizing<String> = Zeroizing::new(passphrase.nfc().collect());
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
pub struct OsKeychain;

impl OsKeychain {
    /// The keychain, or `None` on a platform that has none Beacon can use. Android's
    /// keystore is not wired up yet; there, no profile opens.
    pub fn new() -> Option<Self> {
        cfg!(any(target_os = "linux", target_os = "macos", target_os = "windows")).then_some(Self)
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl OsKeychain {
    fn entry(profile: Uuid, slot: Slot) -> Result<keyring::Entry> {
        let account = match slot {
            Slot::Current => format!("profile-{profile}"),
            Slot::Next => format!("profile-{profile}.next"),
        };
        keyring::Entry::new(KEYCHAIN_SERVICE, &account).context("opening the keychain")
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
impl KeyStore for OsKeychain {
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
        let entry = Self::entry(profile, slot)?;
        // Read as text, the way `set` writes it: on Windows a password is filed as UTF-16,
        // and the raw blob would not be the hex that went in.
        match entry.get_password() {
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
        Self::entry(key.profile, slot)?
            .set_password(&key.filed())
            .context("storing the profile key in the keychain")
    }

    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()> {
        match Self::entry(profile, slot)?.delete_credential() {
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
    // Byte offsets below: anything but ASCII could split a character and panic.
    if !s.is_ascii() || !s.len().is_multiple_of(2) {
        bail!("not even-length ASCII hex");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| anyhow!("not hex: {s:?}")))
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
    fn a_passphrase_opens_however_its_characters_are_composed() {
        let id = Uuid::new_v4();
        let key = ProfileKey::generate(id);
        // "café" with a precomposed é, then with e plus a combining acute.
        let wrapped = key.wrap("caf\u{e9}", TEST_KDF).unwrap();
        let back = ProfileKey::unwrap(id, &wrapped, "cafe\u{301}").unwrap();
        assert_eq!(back.check(), key.check());
        assert!(ProfileKey::unwrap(id, &wrapped, "cafe").is_err());
    }

    #[test]
    fn a_wrapped_key_asking_too_much_is_refused_before_hashing() {
        let id = Uuid::new_v4();
        let mut wrapped = ProfileKey::generate(id).wrap("pw", TEST_KDF).unwrap();
        wrapped.memory_kib = u32::MAX;
        let err = ProfileKey::unwrap(id, &wrapped, "pw").unwrap_err();
        assert!(err.to_string().contains("beyond what Beacon accepts"), "{err}");
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
    }
}
