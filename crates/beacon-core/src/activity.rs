//! What the engine is doing right now, as a handful of one-line status entries with a
//! running clock: "Resolving gosub.io   0.2s". For watching where a page load spends
//! its time, not for logging; lines appear, tick, and go away.
//!
//! Two sources feed it. The engine's telemetry bus announces every timed stage as it
//! starts and stops (`timing.start` / `timing.stop`: parsing, render tree, layout, tiling,
//! raster, paint) and every out-of-process render pass (`remote.*.start` / `remote.*`).
//! The network side comes from the request log the developer panel already keeps: the
//! document request gets a line of its own that follows its phase, and the page's
//! subresources are folded into one line, because forty "Receiving image" entries would
//! say nothing. The frontend drains both into [`Activity`] on a timer and shows
//! [`Activity::lines`].
//!
//! Slots are sticky: an entry keeps its line until it is done, then stays for a moment
//! showing its final time, then frees the slot. When more is going on than fits, the
//! oldest entries keep their lines (the slow ones are what you are watching for) and the
//! last line says how many more there are.

use std::collections::HashMap;
use std::time::{Duration, Instant};

/// How many lines the strip shows.
pub const MAX_LINES: usize = 4;
/// How long a finished entry stays visible with its final time; a blink of a stage stays
/// a shorter while, so forty stylesheet parses do not queue up behind one another.
const LINGER: Duration = Duration::from_millis(1200);
const LINGER_BRIEF: Duration = Duration::from_millis(300);
const BRIEF: Duration = Duration::from_millis(100);
/// Width of the text part of a line; the clock sits right of it.
const LABEL_WIDTH: usize = 60;

/// What an entry is about, so a start and its end meet.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A timed engine stage, by the engine's timer id.
    Stage(String),
    /// An out-of-process render pass: the tab and the pass kind (`remote.scroll`, ...).
    Remote(String, String),
    /// The document request of a navigation, by request id.
    Document(uuid::Uuid),
    /// Everything else the page is fetching, as one entry.
    Subresources,
}

#[derive(Debug)]
struct Entry {
    key: Key,
    label: String,
    started: Instant,
    /// Set once the entry finished: when, and how long it took.
    done: Option<(Instant, Duration)>,
    /// The line it is shown on, once it has one.
    slot: Option<usize>,
}

/// The live set of entries. Feed it with [`Activity::begin`] / [`Activity::end`], or the
/// higher-level [`Activity::on_telemetry`] and [`Activity::sync_requests`], then read
/// [`Activity::lines`] on every tick.
#[derive(Debug, Default)]
pub struct Activity {
    entries: Vec<Entry>,
    slots_taken: HashMap<usize, ()>,
}

impl Activity {
    /// Start showing `label` under `key`; a key already shown just gets the new label.
    pub fn begin(&mut self, key: Key, label: impl Into<String>, now: Instant) {
        let label = label.into();
        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key && e.done.is_none()) {
            entry.label = label;
            return;
        }
        let slot = self.free_slot();
        if let Some(slot) = slot {
            self.slots_taken.insert(slot, ());
        }
        self.entries.push(Entry {
            key,
            label,
            started: now,
            done: None,
            slot,
        });
    }

    /// Change the text of an entry in flight without touching its clock.
    pub fn relabel(&mut self, key: &Key, label: impl Into<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|e| &e.key == key && e.done.is_none()) {
            entry.label = label.into();
        }
    }

    /// The entry under `key` is finished; it stays visible for a moment with its total.
    /// `took` overrides the clock when the source measured the duration itself.
    pub fn end(&mut self, key: &Key, now: Instant, took: Option<Duration>) {
        if let Some(index) = self.entries.iter().position(|e| &e.key == key && e.done.is_none()) {
            // Finished before it was ever shown: nothing to linger.
            if self.entries[index].slot.is_none() {
                self.entries.remove(index);
                return;
            }
            let entry = &mut self.entries[index];
            entry.done = Some((now, took.unwrap_or_else(|| now.duration_since(entry.started))));
        }
    }

    /// Whether `key` is currently in flight.
    pub fn is_active(&self, key: &Key) -> bool {
        self.entries.iter().any(|e| &e.key == key && e.done.is_none())
    }

    /// The lines to show, always `MAX_LINES` of them with an empty string where a slot is
    /// free, so the strip keeps one height and a line keeps its place. Also retires
    /// entries whose linger is over and hands their slots to waiting ones.
    pub fn lines(&mut self, now: Instant) -> Vec<String> {
        self.retire(now);
        let waiting = self.entries.iter().filter(|e| e.slot.is_none()).count();
        let mut lines = vec![String::new(); MAX_LINES];
        let last_slot = self.entries.iter().filter_map(|e| e.slot).max().unwrap_or(0);
        for entry in &self.entries {
            let Some(slot) = entry.slot else { continue };
            let mut label = entry.label.clone();
            if waiting > 0 && slot == last_slot {
                label.push_str(&format!(" (+{waiting} more)"));
            }
            let secs = match entry.done {
                Some((_, took)) => took,
                None => now.duration_since(entry.started),
            }
            .as_secs_f64();
            lines[slot] = format!(
                "{:<width$} {:>5.1}s",
                middle_ellipsis(&label, LABEL_WIDTH),
                secs,
                width = LABEL_WIDTH
            );
        }
        lines
    }

    /// An event from the engine's telemetry bus.
    pub fn on_telemetry(&mut self, kind: &str, data: &serde_json::Value, now: Instant) {
        let text = |field: &str| data.get(field).and_then(|v| v.as_str()).map(str::to_string);
        match kind {
            "timing.start" => {
                let (Some(timer), Some(namespace)) = (text("timer"), text("namespace")) else {
                    return;
                };
                if let Some(label) = stage_label(&namespace, text("context").as_deref()) {
                    self.begin(Key::Stage(timer), label, now);
                }
            }
            "timing.stop" => {
                let Some(timer) = text("timer") else { return };
                let took = data.get("duration_us").and_then(|v| v.as_u64()).map(Duration::from_micros);
                self.end(&Key::Stage(timer), now, took);
            }
            kind if kind.starts_with("remote.") => {
                let tab = text("tab").unwrap_or_default();
                match kind.strip_suffix(".start") {
                    Some(pass) => {
                        let what = pass.strip_prefix("remote.").unwrap_or(pass);
                        self.begin(
                            Key::Remote(tab, pass.to_string()),
                            format!("Rendering {what} in the site's process"),
                            now,
                        );
                    }
                    None => {
                        let key = Key::Remote(tab, kind.to_string());
                        if let Some(laps) = renderer_laps(data) {
                            let what = kind.strip_prefix("remote.").unwrap_or(kind);
                            self.relabel(&key, format!("Rendered {what} in the site's process: {laps}"));
                        }
                        let took = data.get("exchange_us").and_then(|v| v.as_u64()).map(Duration::from_micros);
                        self.end(&key, now, took);
                    }
                }
            }
            _ => {}
        }
    }

    /// Bring the network lines in step with the developer panel's request log: the
    /// document request on its own line, everything else folded into one.
    pub fn sync_requests(&mut self, requests: &[crate::devtools::NetRequest], now: Instant) {
        use crate::devtools::Phase;

        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let since = |started_ms: u64| now - Duration::from_millis(now_ms.saturating_sub(started_ms));

        // Document requests, one entry each; the entry ends when the request does.
        for request in requests.iter().filter(|r| r.kind == "document") {
            let key = Key::Document(request.id);
            let host = host_of(&request.url);
            match request.phase() {
                Phase::Done => {
                    if self.is_active(&key) {
                        let label = match request.state {
                            crate::devtools::RequestState::Finished => {
                                format!("Loaded {host} ({})", crate::devtools::format_bytes(request.received_bytes))
                            }
                            crate::devtools::RequestState::Cancelled => format!("Cancelled {host}"),
                            _ => format!("Failed {host}: {}", request.error.clone().unwrap_or_default()),
                        };
                        self.relabel(&key, label);
                        self.end(&key, now, request.elapsed_us.map(Duration::from_micros));
                    }
                }
                phase => {
                    let label = match phase {
                        Phase::Queued => format!("Queued {host}"),
                        Phase::Opening => format!("Resolving {host}"),
                        Phase::Connecting => format!("Connecting to {host}"),
                        Phase::Waiting => format!("Waiting for {host}"),
                        _ => format!(
                            "Receiving page from {host} ({})",
                            crate::devtools::format_bytes(request.received_bytes)
                        ),
                    };
                    if self.is_active(&key) {
                        self.relabel(&key, label);
                    } else {
                        self.begin(key.clone(), label, since(request.started_ms));
                        if let Some(entry) = self.entries.iter_mut().find(|e| e.key == key) {
                            entry.started = since(request.started_ms);
                        }
                    }
                }
            }
        }

        // Subresources: one line while any is in flight.
        let running: Vec<&crate::devtools::NetRequest> = requests
            .iter()
            .filter(|r| r.kind != "document" && !matches!(r.phase(), Phase::Done))
            .collect();
        if running.is_empty() {
            if self.is_active(&Key::Subresources) {
                let finished = requests
                    .iter()
                    .filter(|r| r.kind != "document" && matches!(r.state, crate::devtools::RequestState::Finished))
                    .count();
                self.relabel(&Key::Subresources, format!("Fetched {finished} resources"));
                self.end(&Key::Subresources, now, None);
            }
            return;
        }
        let count = |wanted: Phase| running.iter().filter(|r| r.phase() == wanted).count();
        let mut parts = Vec::new();
        for (phase, word) in [
            (Phase::Queued, "queued"),
            (Phase::Opening, "resolving"),
            (Phase::Connecting, "connecting"),
            (Phase::Waiting, "waiting"),
            (Phase::Receiving, "receiving"),
        ] {
            let n = count(phase);
            if n > 0 {
                parts.push(format!("{n} {word}"));
            }
        }
        let label = format!("Fetching {} resources: {}", running.len(), parts.join(", "));
        if self.is_active(&Key::Subresources) {
            self.relabel(&Key::Subresources, label);
        } else {
            let earliest = running.iter().map(|r| r.started_ms).min().unwrap_or(now_ms);
            self.begin(Key::Subresources, label, since(earliest));
        }
    }

    fn free_slot(&self) -> Option<usize> {
        (0..MAX_LINES).find(|slot| !self.slots_taken.contains_key(slot))
    }

    /// Drop entries whose linger is over, then seat waiting entries in the freed slots.
    fn retire(&mut self, now: Instant) {
        let mut freed = Vec::new();
        self.entries.retain(|e| match e.done {
            Some((at, took)) if now.duration_since(at) >= if took < BRIEF { LINGER_BRIEF } else { LINGER } => {
                if let Some(slot) = e.slot {
                    freed.push(slot);
                }
                false
            }
            _ => true,
        });
        for slot in freed {
            self.slots_taken.remove(&slot);
        }
        for entry in self.entries.iter_mut().filter(|e| e.slot.is_none()) {
            let Some(slot) = (0..MAX_LINES).find(|slot| !self.slots_taken.contains_key(slot)) else {
                break;
            };
            self.slots_taken.insert(slot, ());
            entry.slot = Some(slot);
        }
    }
}

/// The words for a timed engine stage, or `None` for stages that run per frame and would
/// only flicker (hover and scroll repaints, compositing) or that wrap other stages.
fn stage_label(namespace: &str, context: Option<&str>) -> Option<String> {
    let base = match namespace {
        "html.document" => "Parsing the document",
        "html5.parse" => "Parsing HTML",
        "decode.html" => "Decoding HTML",
        "decode.css" => "Parsing CSS",
        "decode.image" => "Decoding an image",
        "script.blocked_on_css" => "Script waiting for CSS",
        "pipeline.render_tree" => "Building the render tree",
        "pipeline.layout" => "Laying out",
        "pipeline.layering" => "Layering",
        "pipeline.tiling" => "Tiling",
        "pipeline.rasterize" => "Rasterizing",
        "pipeline.painting" => "Painting",
        "pipeline.total" | "pipeline.composite" => return None,
        other
            if other.starts_with("hover.")
                || other.starts_with("gputile.")
                || other.starts_with("pipeline.extend.")
                || other.starts_with("pipeline.hover.")
                || other.starts_with("page.") =>
        {
            return None
        }
        // A stage this build does not know: show it by name rather than hide it.
        other => other,
    };
    Some(match context {
        Some(context) if !context.is_empty() => format!("{base} for {context}"),
        _ => base.to_string(),
    })
}

/// The renderer's own lap times from a completed remote pass, largest first, at most
/// three: "layout 12 ms, raster 8 ms".
fn renderer_laps(data: &serde_json::Value) -> Option<String> {
    let laps = data.get("renderer_us")?.as_object()?;
    let mut laps: Vec<(&str, u64)> = laps
        .iter()
        .filter_map(|(name, us)| us.as_u64().map(|us| (name.as_str(), us)))
        .collect();
    if laps.is_empty() {
        return None;
    }
    laps.sort_by_key(|lap| std::cmp::Reverse(lap.1));
    Some(
        laps.iter()
            .take(3)
            .map(|(name, us)| format!("{} {}", name.trim_start_matches("render."), format_ms(*us)))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn format_ms(us: u64) -> String {
    if us >= 1000 {
        format!("{} ms", us / 1000)
    } else {
        format!("{us} us")
    }
}

/// The host of a URL, or the URL itself when it has none.
fn host_of(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| url.to_string())
}

/// Shorten `text` to `width` characters by cutting the middle, which keeps a URL's host
/// and its file name.
fn middle_ellipsis(text: &str, width: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= width || width < 5 {
        return text.to_string();
    }
    let head = (width - 1) / 2;
    let tail = width - 1 - head;
    let mut out: String = chars[..head].iter().collect();
    out.push('…');
    out.extend(chars[chars.len() - tail..].iter());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> Instant {
        // A fixed origin keeps the tests independent of the real clock.
        thread_local! { static ORIGIN: Instant = Instant::now(); }
        ORIGIN.with(|o| *o + Duration::from_millis(ms))
    }

    #[test]
    fn a_line_shows_its_slot_label_and_running_time() {
        let mut activity = Activity::default();
        activity.begin(Key::Stage("a".into()), "Laying out", at(0));
        let lines = activity.lines(at(1500));
        assert!(lines[0].starts_with("Laying out"), "{}", lines[0]);
        assert!(lines[0].ends_with("  1.5s"), "{}", lines[0]);
        assert_eq!(lines[1], "", "a free slot is a blank line");
        assert_eq!(lines.len(), MAX_LINES);
    }

    #[test]
    fn a_finished_entry_freezes_its_time_then_leaves() {
        let mut activity = Activity::default();
        let key = Key::Stage("a".into());
        activity.begin(key.clone(), "Laying out", at(0));
        activity.end(&key, at(300), None);
        assert!(activity.lines(at(1000))[0].ends_with("  0.3s"), "frozen at its total");
        assert_eq!(activity.lines(at(300 + 1300))[0], "", "gone after the linger");
    }

    #[test]
    fn slots_are_sticky_while_others_come_and_go() {
        let mut activity = Activity::default();
        activity.begin(Key::Stage("a".into()), "A", at(0));
        activity.begin(Key::Stage("b".into()), "B", at(0));
        activity.end(&Key::Stage("a".into()), at(10), None);
        let _ = activity.lines(at(2000)); // A retires, S1 frees
        activity.begin(Key::Stage("c".into()), "C", at(2000));
        let lines = activity.lines(at(2000));
        assert!(lines[0].starts_with("C "), "{}", lines[0]);
        assert!(lines[1].starts_with("B "), "B kept its slot: {}", lines[1]);
    }

    #[test]
    fn overflow_keeps_the_oldest_and_counts_the_rest() {
        let mut activity = Activity::default();
        for i in 0..6 {
            activity.begin(Key::Stage(i.to_string()), format!("Stage {i}"), at(i));
        }
        let lines = activity.lines(at(100));
        assert!(lines[0].starts_with("Stage 0"));
        assert!(lines[3].contains("Stage 3 (+2 more)"), "{}", lines[3]);
    }

    #[test]
    fn an_entry_that_finishes_before_it_is_shown_leaves_no_trace() {
        let mut activity = Activity::default();
        for i in 0..5 {
            activity.begin(Key::Stage(i.to_string()), format!("Stage {i}"), at(i));
        }
        activity.end(&Key::Stage("4".into()), at(10), None); // never had a slot
        let lines = activity.lines(at(20));
        assert!(!lines[3].contains("more"), "{}", lines[3]);
        assert_eq!(lines.len(), 4);
    }

    #[test]
    fn a_brief_stage_lingers_only_briefly() {
        let mut activity = Activity::default();
        let key = Key::Stage("a".into());
        activity.begin(key.clone(), "Parsing CSS", at(0));
        activity.end(&key, at(20), None);
        assert!(activity.lines(at(20 + 200))[0].ends_with("  0.0s"), "still shown with its total");
        assert_eq!(activity.lines(at(20 + 350))[0], "", "a 20 ms stage is gone after the brief linger");
    }

    #[test]
    fn telemetry_stages_pair_start_and_stop_by_timer() {
        let mut activity = Activity::default();
        activity.on_telemetry(
            "timing.start",
            &serde_json::json!({"timer": "t1", "namespace": "pipeline.layout", "context": "https://example.test/"}),
            at(0),
        );
        assert!(activity.lines(at(50))[0].contains("Laying out for https://example.test/"));
        activity.on_telemetry("timing.stop", &serde_json::json!({"timer": "t1", "duration_us": 260_000}), at(50));
        assert!(activity.lines(at(60))[0].ends_with("  0.3s"), "uses the measured duration");
    }

    #[test]
    fn per_frame_stages_are_not_shown() {
        assert_eq!(stage_label("hover.hit_test", None), None);
        assert_eq!(stage_label("pipeline.extend.rasterize", None), None);
        assert_eq!(stage_label("pipeline.total", None), None);
        assert_eq!(stage_label("pipeline.layout", None).as_deref(), Some("Laying out"));
        assert_eq!(stage_label("future.stage", None).as_deref(), Some("future.stage"));
    }

    #[test]
    fn a_remote_pass_shows_in_flight_then_its_laps() {
        let mut activity = Activity::default();
        activity.on_telemetry("remote.scroll.start", &serde_json::json!({"tab": "t", "url": "u"}), at(0));
        assert!(activity.lines(at(10))[0].contains("Rendering scroll in the site's process"));
        activity.on_telemetry(
            "remote.scroll",
            &serde_json::json!({"tab": "t", "exchange_us": 40_000, "renderer_us": {"render.raster": 30_000, "render.paint": 5_000}}),
            at(40),
        );
        let line = activity.lines(at(50))[0].clone();
        assert!(line.contains("raster 30 ms, paint 5 ms"), "{line}");
        assert!(line.ends_with("  0.0s"), "{line}");
    }

    #[test]
    fn long_labels_keep_both_ends() {
        let text = format!("Receiving page from {}/{}", "a".repeat(40), "b".repeat(40));
        let short = middle_ellipsis(&text, 30);
        assert_eq!(short.chars().count(), 30);
        assert!(short.starts_with("Receiving page"));
        assert!(short.ends_with("bbbb"));
    }
}
