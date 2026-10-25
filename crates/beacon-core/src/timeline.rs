//! One navigation on a time axis: the Timeline tab.
//!
//! The Timings tab answers "where does the engine spend its time" across everything it ever
//! did. This answers it for one page load: its requests and the engine's stages side by side
//! on one clock, with the moments that matter -- first bytes, first paint, load -- drawn
//! across them. The last few loads of each tab are kept so two can be compared.
//!
//! Three sources meet here. The tab's navigation events give the milestones Beacon sees for
//! itself. The engine's telemetry bus gives the stages (`timing.start` / `timing.stop`), with
//! wall-clock stamps that line up with the request log's. And the engine's per-navigation
//! timing scope gives the marks only the engine can place: DOM complete, load complete and
//! first paint. The bus only announces stages while someone listens, so a load that ran with
//! the panel closed has its requests and milestones but no stage bars.

use crate::devtools::NetRequest;
use crate::tab::TabId;
use std::collections::HashMap;
use std::sync::Mutex;

/// Loads kept per tab, newest last.
const KEPT: usize = 5;
/// Stages kept per load. Generous for a page; a page that is still repainting a minute later
/// has stopped being a page load.
const STAGE_CAPACITY: usize = 2000;
/// How far apart a load's start and its document request may be heard and still be paired.
const DOCUMENT_SLACK_MS: u64 = 2000;
/// How long after a load finishes its stages are still its own: first paint and the layout
/// that follows it come after `Finished`.
const SETTLE_MS: u64 = 2000;

/// One timed stage of engine work.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    /// The engine's stage name, `html.parse`, `layout`, ...
    pub namespace: String,
    /// What the stage was working on, usually a URL.
    pub context: Option<String>,
    pub start_ms: f64,
    /// `None` while it runs.
    pub end_ms: Option<f64>,
}

/// One page load.
#[derive(Clone, Debug, PartialEq)]
pub struct Navigation {
    pub id: uuid::Uuid,
    pub url: String,
    pub started_ms: u64,
    pub first_bytes_ms: Option<u64>,
    /// When it finished, failed or was cancelled.
    pub ended_ms: Option<u64>,
    /// `None` for a load that finished; otherwise how it ended.
    pub outcome: Option<String>,
    pub stages: Vec<Stage>,
}

impl Navigation {
    /// Whether a stage starting at `at_ms` belongs to this load.
    fn covers(&self, at_ms: u64) -> bool {
        at_ms >= self.started_ms && self.ended_ms.is_none_or(|end| at_ms <= end + SETTLE_MS)
    }
}

#[derive(Default)]
struct Store {
    tabs: HashMap<TabId, Vec<Navigation>>,
    /// Running stages by timer id: which load each went to and where. A stop carries only
    /// the id.
    running: HashMap<String, (TabId, uuid::Uuid, usize)>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

fn with_store<T>(f: impl FnOnce(&mut Store) -> T) -> Option<T> {
    let mut store = STORE.lock().ok()?;
    Some(f(store.get_or_insert_with(Store::default)))
}

fn with_navigation(tab: TabId, id: uuid::Uuid, f: impl FnOnce(&mut Navigation)) {
    with_store(|store| {
        if let Some(nav) = store.tabs.get_mut(&tab).and_then(|navs| navs.iter_mut().find(|n| n.id == id)) {
            f(nav);
        }
    });
}

/// A load began in `tab`.
pub fn started(tab: TabId, id: uuid::Uuid, url: &str, now_ms: u64) {
    with_store(|store| {
        // A reload of the same navigation id starts it again, and a stage still running in
        // the old one must not be closed against the new one's list.
        store.running.retain(|_, (_, nav, _)| *nav != id);
        let navs = store.tabs.entry(tab).or_default();
        navs.retain(|n| n.id != id);
        if navs.len() >= KEPT {
            // The engine's timing scope for it stays: clearing it would take its samples
            // out of the Timings tab's totals too.
            let gone = navs.remove(0).id;
            store.running.retain(|_, (_, nav, _)| *nav != gone);
        }
        navs.push(Navigation {
            id,
            url: url.to_string(),
            started_ms: now_ms,
            first_bytes_ms: None,
            ended_ms: None,
            outcome: None,
            stages: Vec::new(),
        });
    });
}

/// The document's first bytes arrived.
pub fn progressed(tab: TabId, id: uuid::Uuid, now_ms: u64) {
    with_navigation(tab, id, |nav| {
        nav.first_bytes_ms.get_or_insert(now_ms);
    });
}

/// The load ended: `outcome` is `None` when it finished, or what stopped it.
pub fn ended(tab: TabId, id: uuid::Uuid, outcome: Option<&str>, now_ms: u64) {
    with_navigation(tab, id, |nav| {
        if nav.ended_ms.is_none() {
            nav.ended_ms = Some(now_ms);
            nav.outcome = outcome.map(str::to_string);
        }
    });
}

/// The user stopped `tab`'s load. The engine cancels it without an event of its own, so
/// whatever load is still open ends here.
pub fn stopped(tab: TabId, now_ms: u64) {
    with_store(|store| {
        if let Some(nav) = store
            .tabs
            .get_mut(&tab)
            .and_then(|navs| navs.iter_mut().rev().find(|n| n.ended_ms.is_none()))
        {
            nav.ended_ms = Some(now_ms);
            nav.outcome = Some("cancelled".to_string());
        }
    });
}

/// Forget a closed tab's loads.
pub fn forget(tab: TabId) {
    with_store(|store| {
        store.tabs.remove(&tab);
        store.running.retain(|_, (owner, ..)| *owner != tab);
    });
}

/// An event from the engine's telemetry bus. Only stage starts and stops are read.
///
/// A start goes to the load it happened during. The bus does not say which tab a stage
/// worked for, so when loads in two tabs overlap, the one whose address the stage names
/// wins, and otherwise the one that started last.
pub fn on_telemetry(kind: &str, data: &serde_json::Value, ts_us: u64) {
    let at_ms = ts_us as f64 / 1000.0;
    let timer = data.get("timer").and_then(|t| t.as_str()).map(str::to_string);
    let Some(timer) = timer else { return };
    match kind {
        "timing.start" => {
            let namespace = data.get("namespace").and_then(|n| n.as_str()).unwrap_or("stage").to_string();
            let context = data.get("context").and_then(|c| c.as_str()).map(str::to_string);
            with_store(|store| {
                // Another window already filed it.
                if store.running.contains_key(&timer) {
                    return;
                }
                let candidates = store
                    .tabs
                    .iter_mut()
                    .flat_map(|(tab, navs)| navs.iter_mut().map(move |nav| (*tab, nav)))
                    .filter(|(_, nav)| nav.covers(at_ms as u64));
                let mut best: Option<(TabId, &mut Navigation)> = None;
                for (tab, nav) in candidates {
                    let named = context.as_deref() == Some(nav.url.as_str());
                    let better = match &best {
                        None => true,
                        Some((_, current)) => {
                            let current_named = context.as_deref() == Some(current.url.as_str());
                            (named && !current_named) || (named == current_named && nav.started_ms > current.started_ms)
                        }
                    };
                    if better {
                        best = Some((tab, nav));
                    }
                }
                let Some((tab, nav)) = best else { return };
                if nav.stages.len() >= STAGE_CAPACITY {
                    return;
                }
                nav.stages.push(Stage {
                    namespace,
                    context,
                    start_ms: at_ms,
                    end_ms: None,
                });
                let index = nav.stages.len() - 1;
                let id = nav.id;
                store.running.insert(timer, (tab, id, index));
            });
        }
        "timing.stop" => {
            let duration_ms = data.get("duration_us").and_then(|d| d.as_u64()).map(|us| us as f64 / 1000.0);
            with_store(|store| {
                let Some((tab, id, index)) = store.running.remove(&timer) else {
                    return;
                };
                if let Some(stage) = store
                    .tabs
                    .get_mut(&tab)
                    .and_then(|navs| navs.iter_mut().find(|n| n.id == id))
                    .and_then(|nav| nav.stages.get_mut(index))
                {
                    // The duration is the engine's own measure; the stop's stamp includes
                    // however long the event took to be sent.
                    stage.end_ms = Some(duration_ms.map_or(at_ms, |d| stage.start_ms + d));
                }
            });
        }
        _ => {}
    }
}

/// One kept load, without its stages: what a picker lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Load {
    pub id: uuid::Uuid,
    pub url: String,
    pub started_ms: u64,
}

/// `tab`'s kept loads, oldest first.
///
/// Light on purpose: the panel asks four times a second, and a load can hold thousands of
/// stages. [`navigation`] copies out the one being shown.
pub fn loads(tab: TabId) -> Vec<Load> {
    with_store(|store| {
        store
            .tabs
            .get(&tab)
            .map(|navs| {
                navs.iter()
                    .map(|nav| Load {
                        id: nav.id,
                        url: nav.url.clone(),
                        started_ms: nav.started_ms,
                    })
                    .collect()
            })
            .unwrap_or_default()
    })
    .unwrap_or_default()
}

/// One of `tab`'s kept loads in full.
pub fn navigation(tab: TabId, id: uuid::Uuid) -> Option<Navigation> {
    with_store(|store| store.tabs.get(&tab)?.iter().find(|nav| nav.id == id).cloned()).flatten()
}

/// A moment drawn across the whole timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub label: &'static str,
    pub at_ms: f64,
}

/// What one lane draws.
#[derive(Clone, Debug, PartialEq)]
pub enum LaneKind {
    /// A request: waiting for headers, then the body.
    Request { headers_ms: Option<f64>, failed: bool },
    /// One engine stage, every time it ran during the load.
    Stage,
}

/// One row of the timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub label: String,
    /// Shown on hover: the URL in full, or how often and how long a stage ran.
    pub detail: String,
    pub kind: LaneKind,
    /// `(start, end)` in epoch milliseconds; `end` is `None` while still running.
    pub spans: Vec<(f64, Option<f64>)>,
}

/// One load, laid out.
#[derive(Clone, Debug, PartialEq)]
pub struct Timeline {
    pub start_ms: f64,
    pub end_ms: f64,
    pub lanes: Vec<Lane>,
    pub marks: Vec<Mark>,
}

/// The engine's own marks for a load, from its timing scope, as offsets from its start.
fn engine_marks(id: uuid::Uuid) -> Vec<(&'static str, u64)> {
    let stats = gosub_shared::timing::snapshot_stats_for(gosub_shared::timing::ScopeId(id));
    [
        ("page.dom_complete", "DOM complete"),
        ("page.load_complete", "load complete"),
        ("page.first_paint", "first paint"),
    ]
    .into_iter()
    .filter_map(|(namespace, label)| {
        stats
            .iter()
            .find(|s| s.namespace == namespace && s.count > 0)
            .map(|s| (label, s.min_us))
    })
    .collect()
}

/// Lay out `nav` from `requests` (the tab's request log) and `next_started_ms`, when the
/// tab's next load began: requests after that are the next load's.
pub fn layout(nav: &Navigation, requests: &[NetRequest], next_started_ms: Option<u64>, now_ms: u64) -> Timeline {
    layout_with(nav, requests, next_started_ms, now_ms, &engine_marks(nav.id))
}

fn layout_with(
    nav: &Navigation,
    requests: &[NetRequest],
    next_started_ms: Option<u64>,
    now_ms: u64,
    engine_marks: &[(&'static str, u64)],
) -> Timeline {
    let start = nav.started_ms as f64;
    let mut lanes = Vec::new();

    // The request log is stamped when Beacon hears of a request, and request events travel a
    // different road from navigation events: through the network process, when there is one,
    // they arrive tens of milliseconds behind; in one process they can arrive ahead. Clock
    // times cannot say where one load's requests end, so the log is cut at document
    // requests instead: a load owns its own and everything up to the next one.
    let is_document = |r: &NetRequest| r.kind == "document" && r.initiator == "navigation";
    let document = requests
        .iter()
        .enumerate()
        .filter(|(_, r)| is_document(r) && r.started_ms.abs_diff(nav.started_ms) <= DOCUMENT_SLACK_MS)
        .min_by_key(|(_, r)| (r.url != nav.url, r.started_ms.abs_diff(nav.started_ms)));
    let (ours, lag): (Vec<&NetRequest>, f64) = match document {
        Some((index, doc)) => {
            let until = requests[index + 1..]
                .iter()
                .position(is_document)
                .map_or(requests.len(), |next| index + 1 + next);
            // The document request starts the load by definition, so its start is pinned
            // to the load's and every request moves with it.
            (
                requests[index..until].iter().collect(),
                doc.started_ms as f64 - nav.started_ms as f64,
            )
        }
        // No document request in the log (cleared, or never reported): by the clock.
        None => (
            requests
                .iter()
                .filter(|r| r.started_ms >= nav.started_ms && next_started_ms.is_none_or(|next| r.started_ms < next))
                .collect(),
            0.0,
        ),
    };

    for request in ours {
        let begin = request.started_ms as f64 - lag;
        let end = request.elapsed_us.map(|us| begin + us as f64 / 1000.0);
        let name = url::Url::parse(&request.url)
            .ok()
            .and_then(|url| url.path_segments().and_then(|mut s| s.next_back().map(str::to_string)))
            .filter(|last| !last.is_empty())
            .unwrap_or_else(|| request.url.clone());
        lanes.push(Lane {
            label: format!("{} {name}", request.kind),
            detail: request.url.clone(),
            kind: LaneKind::Request {
                headers_ms: request.headers_ms.map(|ms| ms as f64 - lag),
                failed: request.error.is_some(),
            },
            spans: vec![(begin, end)],
        });
    }

    // One lane per stage, in the order each first ran: the engine's pipeline reads top to
    // bottom that way.
    let mut stage_lanes: Vec<Lane> = Vec::new();
    for stage in &nav.stages {
        let lane = match stage_lanes.iter_mut().position(|lane| lane.label == stage.namespace) {
            Some(index) => &mut stage_lanes[index],
            None => {
                stage_lanes.push(Lane {
                    label: stage.namespace.clone(),
                    detail: String::new(),
                    kind: LaneKind::Stage,
                    spans: Vec::new(),
                });
                stage_lanes.last_mut().expect("just pushed")
            }
        };
        lane.spans.push((stage.start_ms, stage.end_ms));
    }
    for lane in &mut stage_lanes {
        let total: f64 = lane.spans.iter().filter_map(|(s, e)| e.map(|e| e - s)).sum();
        lane.detail = match lane.spans.len() {
            1 => format!("{} · {total:.1} ms", lane.label),
            n => format!("{} · ran {n} times, {total:.1} ms in all", lane.label),
        };
    }
    lanes.extend(stage_lanes);

    let mut marks = Vec::new();
    if let Some(at) = nav.first_bytes_ms {
        marks.push(Mark {
            label: "first bytes",
            at_ms: at as f64,
        });
    }
    if let Some(at) = nav.ended_ms {
        let label = match nav.outcome.as_deref() {
            None => "finished",
            Some(_) => "stopped",
        };
        marks.push(Mark { label, at_ms: at as f64 });
    }
    for (label, offset_us) in engine_marks {
        marks.push(Mark {
            label,
            at_ms: start + *offset_us as f64 / 1000.0,
        });
    }
    marks.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));

    // Something still running stretches the axis to now, but only while the load could still
    // be doing it: a stage whose stop was lost would otherwise grow the axis for as long as
    // the tab stays open.
    let running_until = match nav.ended_ms {
        Some(ended) => now_ms.min(ended + SETTLE_MS),
        None => now_ms,
    } as f64;
    let end = lanes
        .iter()
        .flat_map(|lane| lane.spans.iter().map(|(s, e)| e.unwrap_or(running_until).max(*s)))
        .chain(marks.iter().map(|m| m.at_ms))
        .fold(start + 1.0, f64::max);
    Timeline {
        start_ms: start,
        end_ms: end,
        lanes,
        marks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab() -> TabId {
        TabId::new()
    }

    /// The store is process-wide, and a stage goes to whichever open load covers its time,
    /// so tests that feed it run one at a time: another test's open load would take theirs.
    fn serial() -> std::sync::MutexGuard<'static, ()> {
        static SERIAL: Mutex<()> = Mutex::new(());
        SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn a_tab_keeps_its_last_few_loads() {
        let _serial = serial();
        let tab = tab();
        let ids: Vec<_> = (0..KEPT + 2).map(|_| uuid::Uuid::new_v4()).collect();
        for (i, id) in ids.iter().enumerate() {
            started(tab, *id, "https://a.test/", i as u64);
        }
        let kept: Vec<_> = loads(tab).iter().map(|n| n.id).collect();
        assert_eq!(kept, ids[2..]);
        forget(tab);
        assert!(loads(tab).is_empty());
    }

    #[test]
    fn a_stage_goes_to_the_load_that_names_it_and_its_stop_closes_it() {
        let _serial = serial();
        let (a, b) = (tab(), tab());
        let (nav_a, nav_b) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        started(a, nav_a, "https://a.test/", 1_000);
        started(b, nav_b, "https://b.test/", 1_100);
        let timer = uuid::Uuid::new_v4().to_string();
        on_telemetry(
            "timing.start",
            &serde_json::json!({"timer": timer, "namespace": "html.parse", "context": "https://a.test/"}),
            1_200_000,
        );
        // A second window seeing the same event files nothing new.
        on_telemetry(
            "timing.start",
            &serde_json::json!({"timer": timer, "namespace": "html.parse", "context": "https://a.test/"}),
            1_200_000,
        );
        on_telemetry(
            "timing.stop",
            &serde_json::json!({"timer": timer, "duration_us": 30_000}),
            1_231_000,
        );

        let stages = navigation(a, nav_a).unwrap().stages;
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].end_ms, Some(1_230.0));
        assert!(navigation(b, nav_b).unwrap().stages.is_empty());
        forget(a);
        forget(b);
    }

    #[test]
    fn a_restarted_load_does_not_take_a_stop_meant_for_its_old_run() {
        let _serial = serial();
        let a = tab();
        let nav = uuid::Uuid::new_v4();
        started(a, nav, "https://a.test/", 1_000);
        let old = uuid::Uuid::new_v4().to_string();
        on_telemetry("timing.start", &serde_json::json!({"timer": old, "namespace": "layout"}), 1_010_000);
        started(a, nav, "https://a.test/", 1_020);
        let new = uuid::Uuid::new_v4().to_string();
        on_telemetry("timing.start", &serde_json::json!({"timer": new, "namespace": "paint"}), 1_030_000);
        on_telemetry("timing.stop", &serde_json::json!({"timer": old, "duration_us": 5_000}), 1_040_000);

        let stages = navigation(a, nav).unwrap().stages;
        assert_eq!(stages.len(), 1);
        assert_eq!(stages[0].namespace, "paint");
        assert_eq!(stages[0].end_ms, None, "the old run's stop left the new stage alone");
        forget(a);
    }

    #[test]
    fn stop_ends_the_open_load_and_a_later_failure_does_not_rewrite_it() {
        let _serial = serial();
        let a = tab();
        let nav = uuid::Uuid::new_v4();
        started(a, nav, "https://a.test/", 1_000);
        stopped(a, 1_500);
        ended(a, nav, Some("failed"), 1_600);
        let nav = navigation(a, nav).unwrap();
        assert_eq!((nav.ended_ms, nav.outcome.as_deref()), (Some(1_500), Some("cancelled")));
        forget(a);
    }

    #[test]
    fn a_stage_long_after_the_load_settled_is_nobodys() {
        let _serial = serial();
        let a = tab();
        let nav = uuid::Uuid::new_v4();
        started(a, nav, "https://a.test/", 1_000);
        ended(a, nav, None, 2_000);
        let timer = uuid::Uuid::new_v4().to_string();
        on_telemetry(
            "timing.start",
            &serde_json::json!({"timer": timer, "namespace": "paint"}),
            (2_000 + SETTLE_MS + 1) * 1000,
        );
        assert!(navigation(a, nav).unwrap().stages.is_empty());
        forget(a);
    }

    #[test]
    fn requests_heard_late_are_moved_back_to_the_load_that_made_them() {
        let nav = Navigation {
            id: uuid::Uuid::new_v4(),
            url: "https://a.test/".to_string(),
            started_ms: 1_000,
            first_bytes_ms: None,
            ended_ms: None,
            outcome: None,
            stages: Vec::new(),
        };
        let mut doc = NetRequest::new(uuid::Uuid::new_v4(), None, "https://a.test/", 1_030);
        doc.kind = "document".into();
        doc.initiator = "navigation".into();
        doc.headers_ms = Some(1_035);
        doc.elapsed_us = Some(10_000);
        let mut css = NetRequest::new(uuid::Uuid::new_v4(), None, "https://a.test/s.css", 1_040);
        css.kind = "stylesheet".into();

        let timeline = layout_with(&nav, &[doc, css], None, 1_100, &[]);
        assert_eq!(timeline.lanes[0].spans, [(1_000.0, Some(1_010.0))]);
        assert_eq!(
            timeline.lanes[0].kind,
            LaneKind::Request {
                headers_ms: Some(1_005.0),
                failed: false
            }
        );
        assert_eq!(timeline.lanes[1].spans[0].0, 1_010.0);
    }

    #[test]
    fn the_next_loads_requests_stay_out_even_when_heard_before_it_started() {
        let nav = Navigation {
            id: uuid::Uuid::new_v4(),
            url: "https://a.test/".to_string(),
            started_ms: 1_000,
            first_bytes_ms: None,
            ended_ms: None,
            outcome: None,
            stages: Vec::new(),
        };
        let document = |at: u64| {
            let mut r = NetRequest::new(uuid::Uuid::new_v4(), None, "https://a.test/", at);
            r.kind = "document".into();
            r.initiator = "navigation".into();
            r
        };
        // This load's document heard a millisecond early, the next load's a millisecond
        // before the next load itself.
        let mine = document(999);
        let css = NetRequest::new(uuid::Uuid::new_v4(), None, "https://a.test/s.css", 1_010);
        let next = document(4_999);
        let timeline = layout_with(&nav, &[mine, css, next], Some(5_000), 6_000, &[]);
        assert_eq!(timeline.lanes.len(), 2, "{:?}", timeline.lanes);
        assert_eq!(timeline.lanes[0].spans[0].0, 1_000.0);
    }

    #[test]
    fn a_stage_that_never_stopped_does_not_stretch_a_finished_load() {
        let nav = Navigation {
            id: uuid::Uuid::new_v4(),
            url: "https://a.test/".to_string(),
            started_ms: 1_000,
            first_bytes_ms: None,
            ended_ms: Some(1_100),
            outcome: None,
            stages: vec![Stage {
                namespace: "paint".into(),
                context: None,
                start_ms: 1_050.0,
                end_ms: None,
            }],
        };
        let timeline = layout_with(&nav, &[], None, 60_000, &[]);
        assert_eq!(timeline.end_ms, (1_100 + SETTLE_MS) as f64);
    }

    #[test]
    fn layout_puts_requests_first_then_one_lane_per_stage_and_sorts_the_marks() {
        let nav = Navigation {
            id: uuid::Uuid::new_v4(),
            url: "https://a.test/".to_string(),
            started_ms: 1_000,
            first_bytes_ms: Some(1_040),
            ended_ms: Some(1_300),
            outcome: None,
            stages: vec![
                Stage {
                    namespace: "html.parse".into(),
                    context: None,
                    start_ms: 1_050.0,
                    end_ms: Some(1_060.0),
                },
                Stage {
                    namespace: "css.parse".into(),
                    context: None,
                    start_ms: 1_070.0,
                    end_ms: Some(1_075.0),
                },
                Stage {
                    namespace: "css.parse".into(),
                    context: None,
                    start_ms: 1_080.0,
                    end_ms: Some(1_090.0),
                },
            ],
        };
        let mut doc = NetRequest::new(uuid::Uuid::new_v4(), None, "https://a.test/", 1_001);
        doc.kind = "document".into();
        doc.elapsed_us = Some(60_000);
        let mut next = NetRequest::new(uuid::Uuid::new_v4(), None, "https://a.test/other", 2_000);
        next.kind = "document".into();

        let timeline = layout_with(&nav, &[doc, next], Some(1_500), 1_400, &[("first paint", 250_000)]);
        let labels: Vec<_> = timeline.lanes.iter().map(|l| l.label.as_str()).collect();
        assert_eq!(labels, ["document https://a.test/", "html.parse", "css.parse"]);
        assert_eq!(timeline.lanes[2].spans.len(), 2);
        assert_eq!(timeline.lanes[2].detail, "css.parse · ran 2 times, 15.0 ms in all");
        let marks: Vec<_> = timeline.marks.iter().map(|m| (m.label, m.at_ms)).collect();
        assert_eq!(marks, [("first bytes", 1_040.0), ("first paint", 1_250.0), ("finished", 1_300.0)]);
        assert_eq!(timeline.end_ms, 1_300.0);
    }
}
