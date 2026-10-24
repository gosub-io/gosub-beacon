//! `beaconctl`: control Gosub Beacon from the command line.
//!
//! Profiles today. Works on the same folders Beacon uses, so it can run while Beacon is
//! closed or open; anything that would change a profile in use refuses.
//!
//! A profile is named by its name, its id, or its folder (a `--user-data-dir`).

use anyhow::{anyhow, bail, Context, Result};
use beacon_profile::crypto::{KdfParams, KeyStore, OsKeychain, ProfileKey, Slot, Store};
use beacon_profile::{Profile, Roots, DEFAULT_COLLECTION, MANIFEST};
use clap::{Args as ClapArgs, Parser, Subcommand};
use gosub_engine::storage::{is_plaintext_database, SqliteKey};
use rusqlite::Connection;
use std::io::{IsTerminal, Write};
use std::path::Path;
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
    /// Remove a profile for good: its folder, its cache and its key in this keychain.
    /// Refused while Beacon has it open. Given a folder Beacon ran with --user-data-dir,
    /// only the cache and the key go; the folder is yours to delete. Asks for the name
    /// first, unless --yes.
    Remove {
        profile: String,
        #[arg(long)]
        yes: bool,
    },
    /// Open this profile when Beacon starts without --profile.
    SetDefault { profile: String },
    /// Print a profile's folder, for scripts. With --cache, its cache folder.
    Path {
        profile: Option<String>,
        #[arg(long)]
        cache: bool,
    },
    /// Set or change the passphrase, so the profile can be opened on another machine.
    /// Needs the profile's key in this machine's keychain. Changing it re-encrypts the
    /// databases under a new key, so an older copy of the profile folder's manifest no
    /// longer opens them with the passphrase it carried.
    Passphrase {
        profile: Option<String>,
        #[command(flatten)]
        input: PassphraseInput,
    },
    /// Remove the passphrase: the profile then opens only where the keychain holds its key.
    /// Re-encrypts the databases under a new key, like changing it.
    RemovePassphrase { profile: Option<String> },
    /// Remove this machine's copy of the profile key, as before handing the machine on.
    /// Only with a passphrase set, and asked for first: it is then the one way back in.
    Forget {
        profile: Option<String>,
        #[command(flatten)]
        input: PassphraseInput,
    },
    /// Print the pages a profile has visited, most recent first, one `title<TAB>url` per
    /// line. Reads while Beacon has the profile open.
    History {
        profile: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Open a profile copied from another machine: the passphrase unwraps its key, and this
    /// machine's keychain keeps it from then on.
    Unlock {
        profile: Option<String>,
        #[command(flatten)]
        input: PassphraseInput,
    },
    /// Move the profile key to a keychain collection of its own (Linux), which asks for its
    /// own password before Beacon gets the key, or back to the default one. The collection
    /// is created on first use, and the keychain asks for its new password then.
    Keychain {
        profile: String,
        /// The label of the Secret Service collection to keep the key in.
        #[arg(long, conflicts_with = "default", required_unless_present = "default")]
        collection: Option<String>,
        /// Back to the default collection, the login keyring.
        #[arg(long)]
        default: bool,
    },
    /// Write a profile to one file, a .tgz, to bring to another machine with `import`.
    /// Refused while Beacon has the profile open. The archive carries the key wrapped by
    /// the profile's passphrase; a profile without one is asked for a passphrase for the
    /// archive alone.
    Export {
        profile: Option<String>,
        /// Where to write it. Default: `<name>.beacon-profile.tgz` here.
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[command(flatten)]
        input: PassphraseInput,
    },
    /// Bring in a profile from a file `export` wrote. It arrives locked: `unlock` with the
    /// passphrase it was exported with.
    Import {
        archive: PathBuf,
        /// Another name, when one here has its name already.
        #[arg(long)]
        name: Option<String>,
        /// Put it in place of the profile here with the same id, which is lost.
        #[arg(long)]
        replace: bool,
    },
}

#[derive(ClapArgs)]
struct PassphraseInput {
    /// Read the passphrase as one line from stdin instead of asking on the terminal.
    #[arg(long)]
    passphrase_stdin: bool,
}

fn main() {
    // Over ssh there is no screen for the keychain's "beaconctl wants to use your
    // confidential information" dialog, and macOS would wait on it forever. Better a clear
    // error than a hang; the dialog is answered once, in a terminal on the Mac itself.
    if std::env::var_os("SSH_CONNECTION").is_some() {
        OsKeychain::forbid_dialogs();
    }
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
                println!("{mark} {:<20} {}{busy}", printable(p.name()), p.id());
            }
        }
        ProfileCommand::Show { profile } => {
            let p = existing(&roots, profile)?;
            // The manifest travels with a copied profile: what it says is printed, never acted
            // on by the terminal.
            println!("name            {}", printable(p.name()));
            println!("id              {}", p.id());
            println!("folder          {}", p.dir.display());
            println!("cache           {}", p.cache_dir.display());
            println!("layout          {}", p.manifest.schema);
            println!("created by      {}", printable(&p.manifest.created_by));
            println!("last opened by  {}", printable(&p.manifest.last_opened_by));
            println!("in use          {}", if p.in_use() { "yes" } else { "no" });
            match &p.manifest.crypto {
                // Plaintext databases get encrypted on the next start; encrypted ones with
                // no manifest entry and no key here are refused, so promise neither.
                None => println!("encrypted       nothing in the manifest yet"),
                Some(crypto) => {
                    let here = OsKeychain::new()
                        .map(|k| p.key_from(Some(&k.in_collection(p.keychain_collection()))).is_ok())
                        .unwrap_or(false);
                    println!("encrypted       yes");
                    println!("passphrase      {}", if crypto.passphrase.is_some() { "yes" } else { "no" });
                    println!("key here        {}", if here { "yes" } else { "no" });
                }
            }
            println!(
                "keychain        {}",
                printable(p.keychain_collection().unwrap_or(DEFAULT_COLLECTION))
            );
            if let Some(leftover) = p.leftover_key_collection() {
                // Asking for the collection the key is in already only clears the leftover.
                let here = match p.keychain_collection() {
                    Some(collection) => format!("--collection {}", shell_quote(&printable(collection))),
                    None => "--default".to_string(),
                };
                println!(
                    "leftover key    {} (`beaconctl profile keychain {} {here}` removes it)",
                    printable(leftover),
                    p.id()
                );
            }
            for z in &p.manifest.zones {
                println!("zone            {} {}", z.id, printable(&z.name));
            }
        }
        ProfileCommand::Create { name } => {
            let p = roots.create(&name, VERSION)?;
            println!("created {} {}", printable(p.name()), p.id());
        }
        ProfileCommand::Remove { profile, yes } => {
            let p = existing(&roots, Some(profile))?;
            let owned = roots.owns(&p);
            if !yes {
                if !std::io::stdin().is_terminal() {
                    bail!(
                        "removing {} cannot be undone; pass --yes to do it without asking",
                        printable(p.name())
                    );
                }
                if owned {
                    print!("This removes {} and everything in it. ", printable(p.name()));
                } else {
                    print!(
                        "This removes the cache and the key of {}; its folder {} stays, and no longer opens. ",
                        printable(p.name()),
                        p.dir.display()
                    );
                }
                print!("Type its name to go on: ");
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if answer.trim() != p.name() {
                    bail!("not removed: that is not its name");
                }
            }
            let keys = keychain(&p)?;
            let leftover = match p.leftover_key_collection() {
                Some(leftover) => Some(
                    OsKeychain::new()
                        .context("this platform has no keychain Beacon can use")?
                        .in_collection((leftover != DEFAULT_COLLECTION).then_some(leftover)),
                ),
                None => None,
            };
            roots.remove(&p, &keys, leftover.as_ref().map(|k| k as &dyn KeyStore))?;
            if !owned {
                println!(
                    "removed the cache and the key of {}; its folder {} stays",
                    printable(p.name()),
                    p.dir.display()
                );
            } else {
                println!("removed {}", printable(p.name()));
            }
        }
        ProfileCommand::SetDefault { profile } => {
            let p = existing(&roots, Some(profile))?;
            // A running Beacon took its GTK id from whether it is the default: the default
            // has the plain one. Moving the default under it would hand the next plain
            // launch to the wrong profile's window.
            let old_id = roots.default_id()?;
            if old_id != Some(p.id()) {
                let old = match old_id {
                    Some(id) => roots.find(&id.to_string())?,
                    None => None,
                };
                for running in [Some(&p), old.as_ref()].into_iter().flatten() {
                    if running.in_use() {
                        bail!(
                            "profile {} is open in Beacon; close it before changing the default",
                            printable(running.name())
                        );
                    }
                }
            }
            roots.set_default(p.id())?;
            println!("default is now {} {}", printable(p.name()), p.id());
        }
        ProfileCommand::Path { profile, cache } => {
            let p = existing(&roots, profile)?;
            let dir: &PathBuf = if cache { &p.cache_dir } else { &p.dir };
            println!("{}", dir.display());
        }
        ProfileCommand::Passphrase { profile, input } => {
            let mut p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            let keychain = keychain(&p)?;
            // The first passphrase wraps the key as it is: there is no earlier wrap to
            // revoke. A change, or a change that was interrupted, is a new key.
            let first = !p.has_passphrase() && !p.rotation_pending(&keychain)?;
            if first {
                let key = p.key_from(Some(&keychain))?;
                // The passphrase is what makes a copy of the folder safe to carry; nothing in
                // it may still be readable without.
                encrypt_plaintext(&p, &key)?;
                let passphrase = input.read(true)?;
                p.set_passphrase(&key, &passphrase, KdfParams::default())?;
                println!(
                    "passphrase set for {}; it now opens on another machine with `beaconctl profile unlock`",
                    printable(p.name())
                );
            } else {
                let passphrase = input.read(true)?;
                p.rotate_key(&keychain, Some(&passphrase), KdfParams::default(), &mut rekey_store)?;
                println!(
                    "passphrase changed for {}; its databases are re-encrypted under a new key, so an older copy of {MANIFEST} no longer opens them",
                    printable(p.name())
                );
            }
        }
        ProfileCommand::RemovePassphrase { profile } => {
            let mut p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            let keychain = keychain(&p)?;
            if !p.has_passphrase() && !p.rotation_pending(&keychain)? {
                bail!("{} has no passphrase", printable(p.name()));
            }
            p.rotate_key(&keychain, None, KdfParams::default(), &mut rekey_store)?;
            println!(
                "passphrase removed from {}; its databases are re-encrypted under a new key, so it now opens only on this machine, and is lost with this keychain",
                printable(p.name())
            );
        }
        ProfileCommand::Forget { profile, input } => {
            let p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            if !p.has_passphrase() {
                bail!(
                    "{} has no passphrase; without its keychain key nothing could open it again",
                    printable(p.name())
                );
            }
            let keychain = keychain(&p)?;
            // Mid-rotation the passphrase may wrap a key the databases are not all on yet.
            p.ensure_not_rotating(&keychain)?;
            // That a passphrase is set proves nothing; that it still unwraps the key does.
            p.key_from_passphrase(&input.read(false)?)
                .context("the passphrase must open the profile before its key here is dropped")?;
            keychain.delete(p.id(), Slot::Current)?;
            println!(
                "forgot the key for {}; `beaconctl profile unlock` brings it back",
                printable(p.name())
            );
        }
        ProfileCommand::History { profile, limit } => {
            let p = existing(&roots, profile)?;
            let path = p.dir.join(Store::Places.file_name());
            // Opening it would create it; there is no history yet either way.
            if !path.exists() {
                return Ok(());
            }
            let key = p.key_from(Some(&keychain(&p)?))?;
            for (title, url) in recent_visits(&path, &key.store_key(Store::Places), limit)
                .with_context(|| format!("reading the history of {}", printable(p.name())))?
            {
                // Titles come from pages: one record per line, and nothing that a terminal
                // would act on.
                println!("{}\t{}", printable(&title), printable(&url));
            }
        }
        ProfileCommand::Unlock { profile, input } => {
            let p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            let keychain = keychain(&p)?;
            if p.key_from(Some(&keychain)).is_ok() {
                println!("{} already opens on this machine", printable(p.name()));
                return Ok(());
            }
            let passphrase = input.read(false)?;
            p.unlock_with_passphrase(&keychain, &passphrase)?;
            println!("unlocked {}; its key is in this machine's keychain now", printable(p.name()));
        }
        ProfileCommand::Keychain {
            profile,
            collection,
            default: _,
        } => {
            if !OsKeychain::has_collections() {
                bail!("this platform's keychain has no collections to pick from");
            }
            if collection.as_deref() == Some("") {
                bail!("a collection needs a name");
            }
            if collection.as_deref() == Some(DEFAULT_COLLECTION) {
                bail!("{DEFAULT_COLLECTION:?} is the default collection; use --default");
            }
            let mut p = existing(&roots, Some(profile))?;
            let _lock = p.lock()?;
            // A move that stopped before the old copy went: finish it first.
            if let Some(leftover) = p.leftover_key_collection().map(str::to_string) {
                let old = (leftover != DEFAULT_COLLECTION).then_some(leftover.as_str());
                let keys = OsKeychain::new()
                    .context("this platform has no keychain Beacon can use")?
                    .in_collection(old);
                p.remove_leftover_key(&keys)?;
                println!("removed the copy of the key {} left in {leftover:?}", printable(p.name()));
            }
            if p.keychain_collection() == collection.as_deref() {
                println!("{} already keeps its key there", printable(p.name()));
                return Ok(());
            }
            let from = keychain(&p)?;
            let to = OsKeychain::new()
                .context("this platform has no keychain Beacon can use")?
                .in_collection(collection.as_deref());
            p.move_key(&from, &to, collection.as_deref())?;
            match &collection {
                Some(c) => println!(
                    "{} keeps its key in the keychain collection {c:?}; lock that collection and Beacon asks for its password",
                    printable(p.name())
                ),
                None => println!("{} keeps its key in the default keychain collection", printable(p.name())),
            }
        }
        ProfileCommand::Export { profile, output, input } => {
            let p = existing(&roots, profile)?;
            let _lock = p.lock()?;
            if p.manifest.crypto.is_none() {
                bail!(
                    "{} is not encrypted yet; start Beacon with it once, which encrypts it",
                    printable(p.name())
                );
            }
            let output = output.unwrap_or_else(|| PathBuf::from(format!("{}.beacon-profile.tgz", plain_name(&p))));
            if output.exists() {
                bail!("{} exists already", output.display());
            }
            let keychain = keychain(&p)?;
            // Mid-rotation the passphrase may wrap a key the databases are not all on yet.
            p.ensure_not_rotating(&keychain)?;
            let key = match p.key_from(Some(&keychain)) {
                Ok(key) => {
                    // Anything a first start left in plaintext would leave in the archive so.
                    encrypt_plaintext(&p, &key)?;
                    Some(key)
                }
                // A copied profile not unlocked here still carries its passphrase.
                Err(_) if p.has_passphrase() => None,
                Err(e) => return Err(e),
            };
            let passphrase = if p.has_passphrase() {
                None
            } else {
                eprintln!("{} has no passphrase; the archive needs one to carry its key", printable(p.name()));
                Some(input.read(true)?)
            };
            let wrap = key.as_ref().zip(passphrase.as_deref().map(String::as_str));
            write_new(&output, |out| p.export(out, wrap, VERSION))?;
            println!(
                "exported {} to {}; on the other machine: `beaconctl profile import {}`",
                printable(p.name()),
                output.display(),
                shell_quote(&output.display().to_string())
            );
        }
        ProfileCommand::Import { archive, name, replace } => {
            let file = std::fs::File::open(&archive).with_context(|| format!("opening {}", archive.display()))?;
            let p = roots.import(std::io::BufReader::new(file), name.as_deref(), replace, VERSION)?;
            println!("imported {} {}", printable(p.name()), p.id());
            let here = OsKeychain::new()
                .map(|k| p.key_from(Some(&k.in_collection(p.keychain_collection()))).is_ok())
                .unwrap_or(false);
            if here {
                println!("its key is in this machine's keychain already");
            } else {
                println!(
                    "unlock it with the passphrase it was exported with: `beaconctl profile unlock {}`",
                    plain_name(&p)
                );
            }
        }
    }
    Ok(())
}

/// `text` with every control character (newlines, tabs, escape sequences) replaced by a
/// space, so one record stays one line and a page title cannot drive the terminal.
fn printable(text: &str) -> String {
    text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// `text` as one shell word: single-quoted unless it is plain.
fn shell_quote(text: &str) -> String {
    if !text.is_empty() && text.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        text.to_string()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// How a file name or a command names `p`: its name when that is a plain word, which a shell
/// takes as it is, its id otherwise.
fn plain_name(p: &Profile) -> String {
    let name = p.name();
    if !name.is_empty() && !name.starts_with(['-', '.']) && name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) {
        name.to_string()
    } else {
        p.id().to_string()
    }
}

/// Write `path`, which must not exist yet, through `write`: into a private file beside it
/// that is renamed into place once complete, and removed when it is not.
fn write_new(path: &Path, write: impl FnOnce(&mut dyn Write) -> Result<()>) -> Result<()> {
    let mut part = path.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut options = std::fs::File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&part).with_context(|| format!("creating {}", part.display()))?;
    let result = (|| {
        let mut out = std::io::BufWriter::new(file);
        write(&mut out)?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        // Not over a file that appeared meanwhile.
        if path.exists() {
            bail!("{} exists already", path.display());
        }
        std::fs::rename(&part, path).with_context(|| format!("writing {}", path.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// The keychain, looking in the collection `p` keeps its key in.
fn keychain(p: &Profile) -> Result<impl KeyStore> {
    Ok(OsKeychain::new()
        .context("this platform has no keychain Beacon can use")?
        .in_collection(p.keychain_collection()))
}

/// Encrypt every store still in plaintext and seal the small files, as Beacon does on its
/// first keyed start: one cut short leaves some behind. Call with the profile locked.
fn encrypt_plaintext(p: &Profile, key: &ProfileKey) -> Result<()> {
    for store in Store::ALL {
        let path = p.dir.join(store.file_name());
        if is_plaintext_database(&path)? {
            SqliteKey::from_bytes(*key.store_key(store))
                .encrypt_file(&path)
                .map_err(|e| anyhow!("encrypting {}: {e}", path.display()))?;
        }
    }
    p.seal_plaintext_files(key)
}

/// The `limit` most recently visited pages in the places store at `path`, as `(title,
/// url)`. Asked of the database directly: the engine's `query_visited` ranks by visit
/// count, and getting the most recent out of it meant reading every row.
fn recent_visits(path: &Path, key: &Zeroizing<[u8; 32]>, limit: usize) -> Result<Vec<(String, String)>> {
    let conn = Connection::open(path)?;
    SqliteKey::from_bytes(**key).apply(&conn)?;
    let mut stmt = conn.prepare("SELECT title, url FROM visits ORDER BY last_visit DESC LIMIT ?1")?;
    let rows = stmt.query_map([i64::try_from(limit).unwrap_or(i64::MAX)], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Make the store at `path` open with `new`, for [`Profile::rotate_key`]: re-encrypt it when
/// it is on `old`, leave it when it is on `new` already (a rotation picking up where it
/// stopped), and refuse one that is on neither. SQLCipher's `rekey` rewrites every page in
/// one transaction, so a crash during it leaves the file on one key or the other.
fn rekey_store(path: &Path, old: &Zeroizing<[u8; 32]>, new: &Zeroizing<[u8; 32]>) -> Result<()> {
    let new_key = SqliteKey::from_bytes(**new);
    // A store Beacon has not encrypted yet (its first keyed start was cut short) is
    // encrypted here, with the key the rotation ends on: left as it is, it would sit in the
    // folder in plain text with the passphrase set.
    if is_plaintext_database(path)? {
        return new_key
            .encrypt_file(path)
            .map_err(|e| anyhow!("encrypting {}: {e}", path.display()));
    }
    // Each try on a connection of its own: after a key that did not open the file, the
    // connection is not one to set another key on.
    if new_key.apply(&Connection::open(path)?).is_ok() {
        return Ok(());
    }
    let conn = Connection::open(path)?;
    SqliteKey::from_bytes(**old)
        .apply(&conn)
        .map_err(|e| anyhow!("{} opens with neither the profile's key nor the new one: {e}", path.display()))?;
    // Frames a crashed Beacon left in the WAL go into the file first, so the rekey
    // rewrites everything there is and nothing on the old key is left beside it. The first
    // column says whether a reader kept the checkpoint from finishing.
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get(0))?;
    if busy != 0 {
        bail!("{} is being read by another process; close it and try again", path.display());
    }
    let mut sql = Zeroizing::new(String::from("PRAGMA rekey = \"x'"));
    for b in new.iter() {
        sql.push(char::from_digit(u32::from(b >> 4), 16).unwrap_or('0'));
        sql.push(char::from_digit(u32::from(b & 0xf), 16).unwrap_or('0'));
    }
    sql.push_str("'\";");
    conn.execute_batch(&sql)?;
    drop(conn);
    new_key
        .apply(&Connection::open(path)?)
        .map_err(|e| anyhow!("{} does not open with its new key after re-encrypting: {e}", path.display()))?;
    Ok(())
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
        Some(key) if Path::new(&key).join(beacon_profile::MANIFEST).is_file() => return Profile::at(Path::new(&key), &roots.cache),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> Zeroizing<[u8; 32]> {
        Zeroizing::new([n; 32])
    }

    fn opens_with(path: &Path, key: &Zeroizing<[u8; 32]>) -> bool {
        SqliteKey::from_bytes(**key).apply(&Connection::open(path).unwrap()).is_ok()
    }

    #[test]
    fn rekey_moves_a_store_to_the_new_key_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cookies.db");
        {
            let conn = Connection::open(&path).unwrap();
            SqliteKey::from_bytes(*key(1)).apply(&conn).unwrap();
            conn.execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES ('kept');")
                .unwrap();
        }
        rekey_store(&path, &key(1), &key(2)).unwrap();
        assert!(!opens_with(&path, &key(1)));
        assert!(opens_with(&path, &key(2)));
        // Again, as a resumed rotation would: already there, nothing to do.
        rekey_store(&path, &key(1), &key(2)).unwrap();
        let conn = Connection::open(&path).unwrap();
        SqliteKey::from_bytes(*key(2)).apply(&conn).unwrap();
        let v: String = conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "kept");
        // On neither key: refused, and left alone.
        let err = rekey_store(&path, &key(3), &key(4)).unwrap_err();
        assert!(err.to_string().contains("neither"), "{err}");
        assert!(opens_with(&path, &key(2)));
    }

    /// A store in WAL mode with frames not yet in the file, as a crashed Beacon leaves one.
    #[test]
    fn rekey_takes_the_wal_along() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("local-storage.db");
        let writer = Connection::open(&path).unwrap();
        SqliteKey::from_bytes(*key(1)).apply(&writer).unwrap();
        writer.pragma_update(None, "journal_mode", "WAL").unwrap();
        writer
            .execute_batch("CREATE TABLE t(v TEXT); INSERT INTO t VALUES ('in the wal');")
            .unwrap();
        // Still open: SQLite checkpoints on the last close, which has not happened.
        assert!(dir.path().join("local-storage.db-wal").metadata().unwrap().len() > 0);
        rekey_store(&path, &key(1), &key(2)).unwrap();
        drop(writer);
        assert!(!opens_with(&path, &key(1)));
        let conn = Connection::open(&path).unwrap();
        SqliteKey::from_bytes(*key(2)).apply(&conn).unwrap();
        let v: String = conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "in the wal");
    }

    #[test]
    fn rekey_encrypts_a_plaintext_store_under_the_new_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("places.db");
        Connection::open(&path).unwrap().execute_batch("CREATE TABLE t(v);").unwrap();
        rekey_store(&path, &key(1), &key(2)).unwrap();
        assert!(!is_plaintext_database(&path).unwrap());
        assert!(opens_with(&path, &key(2)));
    }

    #[test]
    fn recent_visits_come_newest_first_and_stop_at_the_limit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("places.db");
        {
            let conn = Connection::open(&path).unwrap();
            SqliteKey::from_bytes(*key(1)).apply(&conn).unwrap();
            conn.execute_batch(
                "CREATE TABLE visits (url TEXT PRIMARY KEY, title TEXT, visit_count INTEGER, last_visit INTEGER);
                 INSERT INTO visits VALUES ('a', 'A', 9, 1), ('b', 'B', 1, 3), ('c', 'C', 5, 2);",
            )
            .unwrap();
        }
        let got = recent_visits(&path, &key(1), 2).unwrap();
        assert_eq!(got, [("B".to_string(), "b".to_string()), ("C".to_string(), "c".to_string())]);
    }
}
