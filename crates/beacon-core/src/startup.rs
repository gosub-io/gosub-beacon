//! Why Beacon could not start, put for the person looking at the screen.
//!
//! Opening the profile can fail in ways the user can act on: another Beacon has it, the
//! keychain is not there or not answering, the key lives on another machine. Every shell
//! shows the same words for these, so they are worded once, here.

use beacon_profile::crypto::KeychainError;
use beacon_profile::{InUse, Locked, Rotating};

/// A failed start, in words for a dialog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupError {
    /// What went wrong, in one sentence.
    pub reason: String,
    /// What the user can do about it, when there is something.
    pub hint: Option<String>,
    /// The error as Beacon has it, for a bug report.
    pub detail: String,
}

impl StartupError {
    pub fn from_error(err: &anyhow::Error) -> Self {
        let detail = format!("{err:#}");
        if let Some(in_use) = err.downcast_ref::<InUse>() {
            return Self {
                reason: format!("The profile \"{}\" is already open in another Beacon.", in_use.name),
                hint: Some("Close the other Beacon, then try again.".into()),
                detail,
            };
        }
        if let Some(locked) = err.downcast_ref::<Locked>() {
            let held = if locked.stale {
                "the keychain holds a different key for it"
            } else {
                "this machine's keychain does not have its key"
            };
            let hint = if locked.passphrase {
                format!("Unlock it once with its passphrase, then try again:\n{}", locked.command())
            } else {
                "It has no passphrase, so it can only be opened on the machine that made it.".into()
            };
            return Self {
                reason: format!("The profile \"{}\" is encrypted, and {held}.", locked.name),
                hint: Some(hint),
                detail,
            };
        }
        if let Some(rotating) = err.downcast_ref::<Rotating>() {
            return Self {
                reason: format!("A change of key for the profile \"{}\" was interrupted.", rotating.name),
                hint: Some(format!(
                    "Run the same command again to finish it, then try again:\n{}",
                    rotating.command()
                )),
                detail,
            };
        }
        if err.downcast_ref::<KeychainError>().is_some() {
            return Self {
                reason: "Beacon could not reach the keychain that holds the profile's key.".into(),
                hint: Some(KEYCHAIN_HINT.into()),
                detail,
            };
        }
        Self {
            reason: "Beacon could not start.".into(),
            hint: None,
            detail,
        }
    }
}

#[cfg(target_os = "linux")]
const KEYCHAIN_HINT: &str = "Beacon keeps each profile's key in the Secret Service (GNOME Keyring or KWallet). \
                             Make sure one is installed, running and unlocked, then try again.";
#[cfg(target_os = "macos")]
const KEYCHAIN_HINT: &str = "Beacon keeps each profile's key in the macOS keychain. Unlock your login keychain, then try again.";
#[cfg(target_os = "windows")]
const KEYCHAIN_HINT: &str = "Beacon keeps each profile's key in the Windows Credential Manager. Try again once you are signed in.";
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
const KEYCHAIN_HINT: &str = "Beacon keeps each profile's key in the system keychain, which did not answer. Try again.";

/// Lock and unlock the profile on a thread of its own, so a shell can keep drawing while
/// the keychain takes its time (a Secret Service that never answers holds the call for two
/// minutes). The result arrives on the returned channel. Safe to call again after a
/// failure. A profile whose key could not be read is let go, so `beaconctl` can unlock it
/// in the meantime.
pub fn open_profile() -> std::sync::mpsc::Receiver<anyhow::Result<()>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("beacon-open-profile".into())
        .spawn(move || {
            let _ = tx.send(crate::paths::lock_profile().and_then(|()| crate::paths::unlock_profile()));
        })
        .expect("spawning the profile thread");
    rx
}

/// How long a shell lets the profile open quietly before saying it is waiting.
pub const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Context};

    #[test]
    fn a_profile_in_use_says_to_close_the_other_beacon() {
        let err = anyhow!(InUse {
            name: "work".into(),
            dir: "/p/work".into(),
        });
        let shown = StartupError::from_error(&err);
        assert_eq!(shown.reason, "The profile \"work\" is already open in another Beacon.");
        assert_eq!(shown.hint.as_deref(), Some("Close the other Beacon, then try again."));
        assert!(shown.detail.contains("/p/work"));
    }

    #[test]
    fn a_locked_profile_with_a_passphrase_gives_the_unlock_command() {
        let err = anyhow!(Locked {
            id: uuid::Uuid::nil(),
            name: "work".into(),
            passphrase: true,
            stale: false,
        });
        let shown = StartupError::from_error(&err);
        assert!(shown.reason.contains("does not have its key"));
        assert!(shown.hint.unwrap().ends_with("beaconctl profile unlock work"));
    }

    #[test]
    fn a_locked_profile_without_a_passphrase_cannot_be_opened_here() {
        let err = anyhow!(Locked {
            id: uuid::Uuid::nil(),
            name: "work".into(),
            passphrase: false,
            stale: true,
        });
        let shown = StartupError::from_error(&err);
        assert!(shown.reason.contains("holds a different key"));
        assert!(shown.hint.unwrap().contains("only be opened on the machine that made it"));
    }

    #[test]
    fn an_interrupted_rotation_gives_the_command_that_finishes_it() {
        let err = anyhow!(Rotating {
            id: uuid::Uuid::nil(),
            name: "work".into(),
        });
        let shown = StartupError::from_error(&err);
        assert!(shown.hint.unwrap().ends_with("beaconctl profile passphrase work"));
    }

    /// The keychain error is context on whatever the platform said, and further context
    /// may sit on top of it; it is found anywhere in the chain.
    #[test]
    fn a_keychain_failure_under_more_context_is_still_a_keychain_failure() {
        let err = Err::<(), _>(anyhow!("org.freedesktop.DBus.Error.ServiceUnknown"))
            .context(KeychainError("reading the profile key from the keychain".into()))
            .context("opening profile work")
            .unwrap_err();
        let shown = StartupError::from_error(&err);
        assert_eq!(shown.hint.as_deref(), Some(KEYCHAIN_HINT));
        assert!(shown.detail.contains("ServiceUnknown"));
    }

    #[test]
    fn anything_else_keeps_its_own_words_in_the_detail() {
        let shown = StartupError::from_error(&anyhow!("manifest version 9 is newer than this Beacon"));
        assert_eq!(shown.reason, "Beacon could not start.");
        assert_eq!(shown.hint, None);
        assert_eq!(shown.detail, "manifest version 9 is newer than this Beacon");
    }
}
