//! The macOS keychain, with the item's access list set as it is made.
//!
//! A keychain item trusts the application that created it and no other: when a second one
//! asks, macOS puts up a dialog ("beaconctl wants to use your confidential information
//! stored in ... in your keychain"), and with no screen to put it on, as over ssh, the call
//! waits forever. Beacon.app and beaconctl both need the profile key, so the item is created
//! trusting both: the running binary and whichever of the two is beside it in the bundle
//! (`Contents/MacOS/BeaconMac` and `Contents/MacOS/beaconctl`). A development build has no
//! such sibling and gets the one dialog per binary that any app gets; "Always Allow" ends it.
//!
//! The access list is only ever set at creation. Changing it on an item that exists asks
//! for the keychain password, which would be a worse dialog than the one this avoids.

use super::{KeyStore, ProfileKey, Slot, KEYCHAIN_SERVICE};
use anyhow::{anyhow, Result};
use core_foundation::array::{CFArray, CFArrayRef};
use core_foundation::base::{CFType, CFTypeRef, TCFType};
use core_foundation::data::CFData;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::{CFString, CFStringRef};
use security_framework::base::Error as SecError;
use security_framework::os::macos::access::SecAccess;
use security_framework::os::macos::keychain::SecKeychain;
use security_framework::os::macos::keychain_item::SecKeychainItem;
use security_framework::os::macos::passwords::{find_generic_password, SecKeychainItemPassword};
use security_framework_sys::base::{errSecItemNotFound, SecAccessRef};
use security_framework_sys::item::{
    kSecAttrAccount, kSecAttrLabel, kSecAttrService, kSecClass, kSecClassGenericPassword, kSecUseKeychain, kSecValueData,
};
use security_framework_sys::keychain::SecKeychainSetUserInteractionAllowed;
use security_framework_sys::keychain_item::{SecItemAdd, SecKeychainItemDelete};
use std::ffi::CString;
use std::os::raw::c_char;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use uuid::Uuid;
use zeroize::Zeroizing;

/// The framework's status code; not re-exported by the crates above.
type OSStatus = i32;

/// `errSecInteractionNotAllowed`: the keychain would have to ask on screen, and may not.
const INTERACTION_NOT_ALLOWED: OSStatus = -25308;

/// What the keychain's dialog calls the item, and its label in Keychain Access.
const LABEL: &str = "Gosub Beacon profile key";

/// The binaries that share a profile key, as they lie side by side in the app bundle.
const SIBLINGS: [&str; 2] = ["BeaconMac", "beaconctl"];

#[repr(C)]
struct OpaqueSecTrustedApplicationRef {
    _private: [u8; 0],
}
type SecTrustedApplicationRef = *mut OpaqueSecTrustedApplicationRef;

// Not in security-framework-sys: the access-list half of the keychain API, deprecated by
// Apple since 10.10 and still what a file-based keychain item is made with.
#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecAttrAccess: CFStringRef;
    fn SecAccessCreate(descriptor: CFStringRef, trustedlist: CFArrayRef, access: *mut SecAccessRef) -> OSStatus;
    fn SecTrustedApplicationCreateFromPath(path: *const c_char, app: *mut SecTrustedApplicationRef) -> OSStatus;
}

/// The keychain Beacon files profile keys in: the user's default one, or a given file.
pub struct MacKeychain {
    keychain: Option<SecKeychain>,
}

impl MacKeychain {
    /// The default keychain, normally the login keychain.
    pub fn new() -> Self {
        Self { keychain: None }
    }

    /// Keys in `keychain` rather than the default one; for tests, which must not touch the
    /// login keychain.
    #[cfg(test)]
    fn in_keychain(keychain: SecKeychain) -> Self {
        Self { keychain: Some(keychain) }
    }

    /// Stop the keychain from asking on screen. With no display to ask on (over ssh) a call
    /// that needs the user would otherwise never return; after this it fails, and the error
    /// says to run once on the Mac itself.
    pub fn forbid_dialogs() {
        // The only failure is an internal one, and asking would be worse than failing.
        let _ = unsafe { SecKeychainSetUserInteractionAllowed(0) };
    }

    fn account(profile: Uuid, slot: Slot) -> String {
        match slot {
            Slot::Current => format!("profile-{profile}"),
            Slot::Next => format!("profile-{profile}.next"),
        }
    }

    fn find(&self, account: &str) -> Result<Option<(SecKeychainItemPassword, SecKeychainItem)>> {
        let keychains = self.keychain.as_ref().map(std::slice::from_ref);
        match find_generic_password(keychains, KEYCHAIN_SERVICE, account) {
            Ok(found) => Ok(Some(found)),
            Err(e) if e.code() == errSecItemNotFound => Ok(None),
            Err(e) => Err(describe(e, "reading the profile key from the keychain")),
        }
    }

    /// A new item for `account` holding `secret`, trusting Beacon's binaries.
    fn add(&self, account: &str, secret: &[u8]) -> Result<()> {
        let access = beacon_access()?;
        // SAFETY: the `kSec*` statics are constant CFStrings owned by the framework, read
        // under the get rule, and every value is a live CF object for the dictionary's life.
        let mut pairs: Vec<(CFString, CFType)> = unsafe {
            vec![
                (key(kSecClass), CFString::wrap_under_get_rule(kSecClassGenericPassword).as_CFType()),
                (key(kSecAttrService), CFString::new(KEYCHAIN_SERVICE).as_CFType()),
                (key(kSecAttrAccount), CFString::new(account).as_CFType()),
                (key(kSecAttrLabel), CFString::new(LABEL).as_CFType()),
                (key(kSecValueData), CFData::from_buffer(secret).as_CFType()),
                (key(kSecAttrAccess), access.as_CFType()),
            ]
        };
        if let Some(keychain) = &self.keychain {
            pairs.push((unsafe { key(kSecUseKeychain) }, keychain.as_CFType()));
        }
        let attributes = CFDictionary::from_CFType_pairs(&pairs);
        // SAFETY: a valid attribute dictionary; the result pointer may be null when the new
        // item is not wanted back.
        let status = unsafe { SecItemAdd(attributes.as_concrete_TypeRef(), ptr::null_mut()) };
        cvt(status).map_err(|e| describe(e, "storing the profile key in the keychain"))
    }
}

impl Default for MacKeychain {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyStore for MacKeychain {
    fn get(&self, profile: Uuid, slot: Slot) -> Result<Option<ProfileKey>> {
        match self.find(&Self::account(profile, slot))? {
            Some((secret, _)) => {
                let secret = Zeroizing::new(secret.to_vec());
                Ok(Some(ProfileKey::from_filed(profile, &secret)?))
            }
            None => Ok(None),
        }
    }

    fn set(&self, key: &ProfileKey, slot: Slot) -> Result<()> {
        let account = Self::account(key.profile, slot);
        let filed = key.filed();
        match self.find(&account)? {
            // In place, keeping the access list the item was made with.
            Some((_, mut item)) => item
                .set_password(filed.as_bytes())
                .map_err(|e| describe(e, "storing the profile key in the keychain")),
            None => self.add(&account, filed.as_bytes()),
        }
    }

    fn delete(&self, profile: Uuid, slot: Slot) -> Result<()> {
        if let Some((_, item)) = self.find(&Self::account(profile, slot))? {
            // Not the wrapper's `delete`, which drops the status: a `Next` key that stays
            // behind would keep reporting a rotation in progress.
            // SAFETY: a live item reference from `find`.
            let status = unsafe { SecKeychainItemDelete(item.as_CFTypeRef() as *mut _) };
            cvt(status).map_err(|e| describe(e, "removing the profile key from the keychain"))?;
        }
        Ok(())
    }
}

/// `kSec*` as a dictionary key.
///
/// # Safety
/// `constant` must be one of the framework's constant CFStrings.
unsafe fn key(constant: CFStringRef) -> CFString {
    CFString::wrap_under_get_rule(constant)
}

/// An access list trusting this binary and the Beacon binaries beside it.
fn beacon_access() -> Result<SecAccess> {
    let mut apps = vec![trusted_application(None)?];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for name in SIBLINGS {
                let sibling = dir.join(name);
                if sibling != exe && sibling.is_file() {
                    // A sibling that cannot be described is left out, not fatal: the key
                    // still gets filed, and that binary gets the dialog.
                    if let Ok(app) = trusted_application(Some(&sibling)) {
                        apps.push(app);
                    }
                }
            }
        }
    }
    let apps = CFArray::from_CFTypes(&apps);
    let descriptor = CFString::new(LABEL);
    let mut access: SecAccessRef = ptr::null_mut();
    // SAFETY: valid CF objects in, a reference out that this process then owns.
    let status = unsafe { SecAccessCreate(descriptor.as_concrete_TypeRef(), apps.as_concrete_TypeRef(), &mut access) };
    cvt(status).map_err(|e| describe(e, "describing who may read the profile key"))?;
    Ok(unsafe { SecAccess::wrap_under_create_rule(access) })
}

/// `path` as an application the keychain can trust; `None` is this process.
fn trusted_application(path: Option<&Path>) -> Result<CFType> {
    let path = path.map(|p| CString::new(p.as_os_str().as_bytes())).transpose()?;
    let mut app: SecTrustedApplicationRef = ptr::null_mut();
    // SAFETY: a NUL-terminated path or null for "this application"; a reference out that
    // this process then owns.
    let status = unsafe { SecTrustedApplicationCreateFromPath(path.as_ref().map_or(ptr::null(), |p| p.as_ptr()), &mut app) };
    cvt(status).map_err(|e| describe(e, "describing an application to the keychain"))?;
    Ok(unsafe { CFType::wrap_under_create_rule(app as CFTypeRef) })
}

fn cvt(status: OSStatus) -> std::result::Result<(), SecError> {
    if status == 0 {
        Ok(())
    } else {
        Err(SecError::from_code(status))
    }
}

fn describe(e: SecError, what: &str) -> anyhow::Error {
    if e.code() == INTERACTION_NOT_ALLOWED {
        return anyhow!(
            "{what}: the keychain has to ask on screen, and this session has none; run this once in a terminal on the Mac itself"
        );
    }
    anyhow::Error::new(e).context(what.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use security_framework::os::macos::keychain::CreateOptions;

    /// Against a throwaway keychain file, never the login keychain, and with every dialog
    /// forbidden: the one thing this cannot show is another application being trusted.
    #[test]
    fn keys_round_trip_through_a_keychain_file() {
        MacKeychain::forbid_dialogs();
        let tmp = tempfile::tempdir().unwrap();
        let keychain = CreateOptions::new()
            .password("test")
            .create(tmp.path().join("test.keychain-db"))
            .unwrap();
        let store = MacKeychain::in_keychain(keychain);
        let id = Uuid::new_v4();
        assert!(store.get(id, Slot::Current).unwrap().is_none());

        let key = ProfileKey::generate(id);
        store.set(&key, Slot::Current).unwrap();
        assert_eq!(store.get(id, Slot::Current).unwrap().unwrap().check(), key.check());
        assert!(store.get(id, Slot::Next).unwrap().is_none());

        // A second set updates in place.
        let other = ProfileKey::generate(id);
        store.set(&other, Slot::Current).unwrap();
        assert_eq!(store.get(id, Slot::Current).unwrap().unwrap().check(), other.check());

        store.set(&key, Slot::Next).unwrap();
        assert_eq!(store.get(id, Slot::Next).unwrap().unwrap().check(), key.check());
        store.delete(id, Slot::Next).unwrap();
        store.delete(id, Slot::Next).unwrap();
        assert!(store.get(id, Slot::Next).unwrap().is_none());
        assert!(store.get(id, Slot::Current).unwrap().is_some());
        store.delete(id, Slot::Current).unwrap();
        assert!(store.get(id, Slot::Current).unwrap().is_none());
    }

    #[test]
    fn this_process_is_a_trusted_application() {
        assert!(trusted_application(None).is_ok());
        assert!(beacon_access().is_ok());
    }
}
