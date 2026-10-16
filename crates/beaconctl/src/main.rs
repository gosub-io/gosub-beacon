//! `beaconctl`: control Gosub Beacon from the command line.
//!
//! Profiles today. Works on the same folders Beacon uses, so it can run while Beacon is
//! closed or open; anything that would change a profile in use refuses.

use anyhow::{bail, Result};
use beacon_profile::{Profile, Roots};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

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
    }
    Ok(())
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
