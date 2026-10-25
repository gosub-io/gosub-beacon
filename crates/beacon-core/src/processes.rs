//! The processes this browser runs, as a tree: the Processes tab.
//!
//! Read from `/proc` rather than asked of the engine, because the engine knows its renderers
//! but not everything else under the broker -- the fork server, decoders, a storage service
//! -- and `/proc` knows all of them. The engine fills in what `/proc` cannot say: which site
//! a renderer serves and how many tabs share it. Every engine child names itself in its
//! command line (`gosub: network process`), which is what the rows are labelled from.
//!
//! Linux only. Elsewhere the engine runs in this one process, and the tree is that.

/// What the engine says about one running renderer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RendererInfo {
    pub pid: u32,
    /// Scheme plus registrable domain: `https://example.com`.
    pub site: String,
    pub tabs: usize,
}

/// One row of the tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    /// How far below this browser's own process: 0 for it, 1 for its children.
    pub depth: usize,
    pub name: String,
    /// What it is for, where that is known: a renderer's site and tabs.
    pub detail: String,
    pub rss_kb: Option<u64>,
    pub uptime_s: Option<u64>,
}

/// One process as `/proc` lists it, before the tree is put together.
#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    pid: u32,
    ppid: u32,
    name: String,
    rss_kb: Option<u64>,
    /// Seconds since boot at which it started.
    started_s: Option<u64>,
}

/// The parent pid and start time (in clock ticks since boot) from `/proc/<pid>/stat`.
///
/// The command name in field 2 is in parentheses and may itself hold spaces and
/// parentheses, so the fields are counted from the last `)`.
#[cfg(target_os = "linux")]
fn parse_stat(stat: &str) -> Option<(u32, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = rest.split_whitespace().collect();
    // `rest` starts at field 3 (state): ppid is field 4, starttime field 22.
    Some((fields.get(1)?.parse().ok()?, fields.get(19)?.parse().ok()?))
}

/// A row's label: the title an engine child gave itself, or the program name.
#[cfg(target_os = "linux")]
fn name_from(cmdline: &[u8], comm: &str) -> String {
    let first = cmdline.split(|b| *b == 0).next().unwrap_or_default();
    let first = String::from_utf8_lossy(first);
    if let Some(title) = first.strip_prefix("gosub: ") {
        return title.to_string();
    }
    match first.rsplit('/').next().filter(|name| !name.is_empty()) {
        Some(program) => program.to_string(),
        None => comm.trim().to_string(),
    }
}

/// `entries` as a tree under `root`, depth first, children in pid order.
#[cfg(target_os = "linux")]
fn order(entries: &[Entry], root: u32) -> Vec<(usize, &Entry)> {
    fn walk<'a>(entries: &'a [Entry], pid: u32, depth: usize, out: &mut Vec<(usize, &'a Entry)>) {
        let Some(entry) = entries.iter().find(|e| e.pid == pid) else {
            return;
        };
        out.push((depth, entry));
        let mut children: Vec<u32> = entries.iter().filter(|e| e.ppid == pid && e.pid != pid).map(|e| e.pid).collect();
        children.sort_unstable();
        for child in children {
            walk(entries, child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(entries, root, 0, &mut out);
    out
}

#[cfg(target_os = "linux")]
fn tree_from(entries: &[Entry], root: u32, renderers: &[RendererInfo], tier: Option<&str>, now_s: Option<u64>) -> Vec<Process> {
    order(entries, root)
        .into_iter()
        .map(|(depth, entry)| {
            let detail = match renderers.iter().find(|r| r.pid == entry.pid) {
                Some(r) => {
                    let tabs = if r.tabs == 1 {
                        "1 tab".to_string()
                    } else {
                        format!("{} tabs", r.tabs)
                    };
                    match tier {
                        Some(tier) => format!("{} · {tabs} · {tier}", r.site),
                        None => format!("{} · {tabs}", r.site),
                    }
                }
                None if depth == 0 => "this browser".to_string(),
                None => String::new(),
            };
            Process {
                pid: entry.pid,
                depth,
                name: if depth == 0 { "Beacon".to_string() } else { entry.name.clone() },
                detail,
                rss_kb: entry.rss_kb,
                uptime_s: now_s.zip(entry.started_s).map(|(now, started)| now.saturating_sub(started)),
            }
        })
        .collect()
}

/// This browser's process and everything under it.
///
/// `tier` is the renderers' confinement, as [`tier_label`] words it.
#[cfg(target_os = "linux")]
pub fn tree(renderers: &[RendererInfo], tier: Option<&str>) -> Vec<Process> {
    // What `/proc` counts start times in. USER_HZ is 100 on every architecture Linux
    // supports, whatever the kernel's own tick rate.
    const TICKS_PER_SECOND: u64 = 100;

    let read = |pid: u32, file: &str| std::fs::read(format!("/proc/{pid}/{file}")).ok();

    // Every process's parent first, which is one small read each; the rest is read only for
    // the ones that turn out to be ours. The panel refreshes four times a second.
    let mut parents: Vec<(u32, u32, u64)> = Vec::new();
    for dir in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Some(pid) = dir.file_name().to_str().and_then(|name| name.parse::<u32>().ok()) else {
            continue;
        };
        // Gone between the listing and the read: a process that exited is not a row.
        let Some(stat) = read(pid, "stat") else { continue };
        if let Some((ppid, started)) = parse_stat(&String::from_utf8_lossy(&stat)) {
            parents.push((pid, ppid, started));
        }
    }
    let root = std::process::id();
    let mut ours = vec![root];
    let mut index = 0;
    while index < ours.len() {
        let parent = ours[index];
        ours.extend(
            parents
                .iter()
                .filter(|(pid, ppid, _)| *ppid == parent && *pid != parent)
                .map(|(pid, ..)| *pid),
        );
        index += 1;
    }

    let entries: Vec<Entry> = parents
        .iter()
        .filter(|(pid, ..)| ours.contains(pid))
        .map(|&(pid, ppid, started)| {
            let comm = read(pid, "comm")
                .map(|c| String::from_utf8_lossy(&c).to_string())
                .unwrap_or_default();
            let rss_kb = read(pid, "status").and_then(|status| {
                String::from_utf8_lossy(&status)
                    .lines()
                    .find(|line| line.starts_with("VmRSS:"))
                    .and_then(|line| line.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse().ok())
            });
            Entry {
                pid,
                ppid,
                name: name_from(&read(pid, "cmdline").unwrap_or_default(), &comm),
                rss_kb,
                started_s: Some(started / TICKS_PER_SECOND),
            }
        })
        .collect();
    let now_s = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|uptime| uptime.split('.').next().and_then(|s| s.parse().ok()));
    tree_from(&entries, root, renderers, tier, now_s)
}

#[cfg(not(target_os = "linux"))]
pub fn tree(_renderers: &[RendererInfo], _tier: Option<&str>) -> Vec<Process> {
    vec![Process {
        pid: std::process::id(),
        depth: 0,
        name: "Beacon".to_string(),
        detail: "this browser".to_string(),
        rss_kb: None,
        uptime_s: None,
    }]
}

/// How confined the renderers are, in a few words.
#[cfg(target_os = "linux")]
pub fn tier_label(tier: &gosub_engine::fork_server::protocol::ConfinementTier) -> String {
    use gosub_engine::fork_server::protocol::ConfinementTier;
    match tier {
        ConfinementTier::Full => "fully sandboxed".to_string(),
        ConfinementTier::FontPathsReadable => "sandboxed, fonts readable".to_string(),
        ConfinementTier::Unsupported(reason) => format!("not sandboxed: {reason}"),
    }
}

/// Seconds as `3h 12m`, `4m 05s` or `12s`.
pub fn format_uptime(seconds: u64) -> String {
    match seconds {
        s if s >= 3600 => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
        s if s >= 60 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{s}s"),
    }
}

// What is tested here is reading `/proc`.
#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn entry(pid: u32, ppid: u32, name: &str) -> Entry {
        Entry {
            pid,
            ppid,
            name: name.to_string(),
            rss_kb: Some(1000),
            started_s: Some(10),
        }
    }

    #[test]
    fn stat_fields_are_counted_from_the_last_parenthesis() {
        // A command name with a space and a parenthesis of its own.
        let stat = "4242 (Web (Content) x) S 17 4242 4242 0 -1 4194304 1 0 0 0 0 0 0 0 20 0 1 0 98765 1 2";
        assert_eq!(parse_stat(stat), Some((17, 98765)));
        assert_eq!(parse_stat("garbage"), None);
    }

    #[test]
    fn an_engine_child_is_named_by_its_title() {
        assert_eq!(name_from(b"gosub: network process\0\0", "gosub-net"), "network process");
        assert_eq!(name_from(b"/usr/bin/beacon\0--isolated\0", "beacon"), "beacon");
        assert_eq!(name_from(b"", "kworker\n"), "kworker");
    }

    #[test]
    fn the_tree_is_depth_first_and_skips_strangers() {
        let entries = [
            entry(1, 0, "init"),
            entry(100, 1, "beacon"),
            entry(103, 100, "renderer fork server"),
            entry(101, 100, "network process"),
            entry(200, 103, "renderer-abc123"),
            entry(300, 1, "someone else"),
        ];
        let renderers = [RendererInfo {
            pid: 200,
            site: "https://a.test".to_string(),
            tabs: 2,
        }];
        let tree = tree_from(&entries, 100, &renderers, Some("fully sandboxed"), Some(70));
        let rows: Vec<_> = tree.iter().map(|p| (p.depth, p.name.as_str(), p.detail.as_str())).collect();
        assert_eq!(
            rows,
            [
                (0, "Beacon", "this browser"),
                (1, "network process", ""),
                (1, "renderer fork server", ""),
                (2, "renderer-abc123", "https://a.test · 2 tabs · fully sandboxed"),
            ]
        );
        assert_eq!(tree[0].uptime_s, Some(60));
    }

    #[test]
    fn uptimes_read_short() {
        assert_eq!(format_uptime(12), "12s");
        assert_eq!(format_uptime(245), "4m 05s");
        assert_eq!(format_uptime(3600 * 3 + 720), "3h 12m");
    }

    #[test]
    fn the_live_tree_starts_at_this_process() {
        let tree = tree(&[], None);
        assert_eq!(tree.first().map(|p| p.pid), Some(std::process::id()));
        assert!(tree[0].rss_kb.is_some());
    }
}
