//! `beaconctl`: control Gosub Beacon from the command line.
//!
//! Profiles today. Works on the same folders Beacon uses, so it can run while Beacon is
//! closed or open; anything that would change a profile in use refuses.

use anyhow::{bail, Context, Result};
use beacon_profile::crypto::{KdfParams, KeyStore, OsKeychain};
use beacon_profile::{Profile, Roots};
use clap::{Args as ClapArgs, Parser, Subcommand};
use std::path::PathBuf;
use zeroize::Zeroizing;

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Control Gosub Beacon from the command line.
#[derive(Parser)]
#[command(name = "beaconctl", version)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Manage profiles.
    #[command(subcommand)]
    Profile(ProfileCommand),
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// List the profiles; `*` marks the default, `in use` one Beacon has open.
    List,
    /// Show one profile: id, folders, zones, versions. Without a name, the default.
    Show { profile: Option<String> },
    /// Create an empty profile.
    Create { name: String },
    /// Open this profile when Beacon starts without --profile.
    SetDefault { profile: String },
    /// Print a profile's folder, for scripts. With --cache, its cache folder.
    Path {
        profile: Option<String>,
        #[arg(long)]
        cache: bool,
    },
    /// Set or change the passphrase, so the profile can be opened on another machine.
    /// Needs the profile's key in this machine's keychain.
    Passphrase {
        profile: Option<String>,
        #[command(flatten)]
        input: PassphraseInput,
    },
    /// Remove the passphrase: the profile then opens only where the keychain holds its key.
    RemovePassphrase { profile: Option<String> },
    /// Remove this machine's copy of the profile key, as before handing the machine on.
    /// Only with a passphrase set: the passphrase is then the one way back in.
    Forget { profile: Option<String> },
    /// Open a profile copied from another machine: the passphrase unwraps its key, and this
    /// machine's keychain keeps it from then on.
    Unlock {
        profile: Option<String>,
        #[command(flatten)]
        input: PassphraseInput,
    },
}

#[derive(ClapArgs)]
struct PassphraseInput {
    /// Read the passphrase as one line from stdin instead of asking on the terminal.
    #[arg(long)]
    passphrase_stdin: bool,
}

fn main() {
    if let Err(e) = run(Args::parse()) {
        eprintln!("beaconctl: {e:#}");
        std::process::exit(1);
    }
}

fn run(args: Args) -> Result<()> {
    match args.command {
        Command::Profile(command) => profile(command),
    }
}

fn profile(command: ProfileCommand) -> Result<()> {
    let roots = Roots::platform()?;
    match command {
        ProfileCommand::List => {
            let default = roots.default_id()?;
            let profiles = roots.list()?;
            if profiles.is_empty() {
                println!("no profiles in {}", roots.profiles_dir().display());
            }
            for p in profiles {
                let mark = if Some(p.id()) == default { '*' } else { ' ' };
                let busy = if p.in_use() { "  in use" } else { "" };
                println!("{mark} {:<20} {}{busy}", p.name(), p.id());
            }
        }
        ProfileCommand::Show { profile } => {
            let p = existing(&roots, profile)?;
            println!("name            {}", p.name());
            println!("id              {}", p.id());
            println!("folder          {}", p.dir.display());
            println!("cache           {}", p.cache_dir.display());
            println!("layout          {}", p.manifest.schema);
            println!("created by      {}", p.manifest.created_by);
            println!("last opened by  {}", p.manifest.last_opened_by);
            println!("in use          {}", if p.in_use() { "yes" } else { "no" });
            match &p.manifest.crypto {
                None => println!("encrypted       no"),
                Some(crypto) => {
                    let here = OsKeychain::new().map(|k| p.key_from(Some(&k)).is_ok()).unwrap_or(false);
                    println!("encrypted       yes");
                    println!("passphrase      {}", if crypto.passphrase.is_some() { "yes" } else { "no" });
                    println!("key here        {}", if here { "yes" } else { "no" });
                }
            }
            for z in &p.manifest.zones {
                println!("zone            {} {}", z.id, z.name);
            }
        }
        ProfileCommand::Create { name } => {
            let p = roots.create(&name, VERSION)?;
            println!("created {} {}", p.name(), p.id());
        }
        ProfileCommand::SetDefault { profile } => {
            let p = existing(&roots, Some(profile))?;
            roots.set_default(p.id())?;
            println!("default is now {} {}", p.name(), p.id());
        }
        ProfileCommand::Path { profile, cache } => {
            let p = existing(&roots, profile)?;
            let dir: &PathBuf = if cache { &p.cache_dir } else { &p.dir };
            println!("{}", dir.display());
        }
        ProfileCommand::Passphrase { profile, input } => {
            let mut p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            let keychain = keychain()?;
            let key = p.key_from(Some(&keychain))?;
            let had = p.manifest.crypto.as_ref().is_some_and(|c| c.passphrase.is_some());
            let passphrase = input.read(true)?;
            p.set_passphrase(&key, &passphrase, KdfParams::default())?;
            let verb = if had { "changed" } else { "set" };
            println!(
                "passphrase {verb} for {}; it now opens on another machine with `beaconctl profile unlock`",
                p.name()
            );
        }
        ProfileCommand::RemovePassphrase { profile } => {
            let mut p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            // Proof the profile still opens here once the passphrase is gone.
            p.key_from(Some(&keychain()?))?;
            p.remove_passphrase()?;
            println!(
                "passphrase removed from {}; it now opens only on this machine, and is lost with this keychain",
                p.name()
            );
        }
        ProfileCommand::Forget { profile } => {
            let p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            if !p.manifest.crypto.as_ref().is_some_and(|c| c.passphrase.is_some()) {
                bail!(
                    "{} has no passphrase; without its keychain key nothing could open it again",
                    p.name()
                );
            }
            keychain()?.delete(p.id())?;
            println!("forgot the key for {}; `beaconctl profile unlock` brings it back", p.name());
        }
        ProfileCommand::Unlock { profile, input } => {
            let p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            let keychain = keychain()?;
            if p.key_from(Some(&keychain)).is_ok() {
                println!("{} already opens on this machine", p.name());
                return Ok(());
            }
            let passphrase = input.read(false)?;
            p.unlock_with_passphrase(&keychain, &passphrase)?;
            println!("unlocked {}; its key is in this machine's keychain now", p.name());
        }
    }
    Ok(())
}

fn keychain() -> Result<impl KeyStore> {
    OsKeychain::new().context("this platform has no keychain Beacon can use")
}

impl PassphraseInput {
    /// The passphrase, asked twice on the terminal when it is a new one.
    fn read(&self, new: bool) -> Result<Zeroizing<String>> {
        if self.passphrase_stdin {
            let mut line = Zeroizing::new(String::new());
            std::io::stdin().read_line(&mut line).context("reading the passphrase from stdin")?;
            let trimmed = Zeroizing::new(line.trim_end_matches(['\n', '\r']).to_string());
            return Ok(trimmed);
        }
        let first = Zeroizing::new(rpassword::prompt_password(if new { "New passphrase: " } else { "Passphrase: " })?);
        if new {
            let again = Zeroizing::new(rpassword::prompt_password("Again: ")?);
            if *first != *again {
                bail!("the two passphrases differ");
            }
        }
        Ok(first)
    }
}

/// A profile that must already exist: unlike Beacon, this tool never creates one by
/// looking it up.
fn existing(roots: &Roots, key: Option<String>) -> Result<Profile> {
    let key = match key {
        Some(key) => key,
        None => match roots.default_id()? {
            Some(id) => id.to_string(),
            None => bail!("there is no default profile yet; Beacon makes one on first start"),
        },
    };
    match roots.find(&key)? {
        Some(p) => Ok(p),
        None => bail!("no profile named {key:?}; see `beaconctl profile list`"),
    }
}
