//! Owning and configuring the Gosub engine: zones, profile storage, settings and
//! internal pages.
//!
//! The engine is fully asynchronous and owns networking, cookies, storage and the render
//! pipeline. A browser tab maps onto an engine [`TabHandle`]. Frames arrive through the
//! shared [`DefaultCompositor`] as `ExternalHandle::TileCache`; getting those onto a
//! screen is a frontend's job, not this module's.
//!
//! [`BrowserEngine`] is generic over the engine's render configuration, so the choice of
//! rasterizer (Skia, Vello, Cairo) belongs to whichever frontend constructs it. That is
//! deliberate: the moment this crate names a renderer, every frontend inherits it.

use std::sync::Arc;

use gosub_engine::cookies::SqliteCookieStore;
use gosub_engine::events::EngineEvent;
use gosub_engine::html::RenderConfiguration;
use gosub_engine::places::SqlitePlaces;
use gosub_engine::storage::{InMemorySessionStore, PartitionPolicy, SqliteLocalStore, StorageService};
use gosub_engine::tab::TabHandle;
use gosub_engine::zone::{Zone, ZoneConfig, ZoneId, ZoneServices};
use gosub_engine::GosubEngine;
use gosub_render_pipeline::render::DefaultCompositor;
use tokio::runtime::Runtime;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
use uuid::uuid;

const DEFAULT_ZONE: uuid::Uuid = uuid!("f1234567-abcd-4000-8000-000000000001");

/// Engine TabId, re-exported for callers that need to key on the engine's identifier.
pub type EngineTabId = gosub_engine::tab::TabId;

/// What a frontend's render configuration has to satisfy to be hosted here. The
/// `CompositorSink` is pinned to [`DefaultCompositor`] because that is the frame channel
/// [`BrowserEngine::compositor`] hands out.
pub trait BeaconConfig: RenderConfiguration<CompositorSink = DefaultCompositor> {}
impl<C: RenderConfiguration<CompositorSink = DefaultCompositor>> BeaconConfig for C {}

/// Owns the running engine, a single default zone and the shared compositor.
///
/// Created once per browser window. The engine itself runs on the shared tokio runtime;
/// this struct lives on the GTK main thread.
pub struct BrowserEngine<C: BeaconConfig> {
    /// Bookmarks + visited history store, shared with the engine's zone.
    places: gosub_engine::places::PlacesHandle,
    /// Kept alive so the engine keeps running for the lifetime of the window.
    #[allow(dead_code)]
    engine: GosubEngine<C>,
    zone: Zone<C>,
    /// Shared compositor; clone the `Arc` into draw callbacks to read frames.
    pub compositor: Arc<DefaultCompositor>,
    /// Fires (after `take_redraw_rx`) whenever a new frame is composited.
    redraw_rx: Option<mpsc::UnboundedReceiver<()>>,
    /// Engine event stream. Subscribed before the zone is created (the engine emits
    /// `ZoneCreated` immediately, which fails if no receiver is alive yet).
    event_rx: Option<broadcast::Receiver<EngineEvent>>,
    /// Per-resource fetch events, on their own stream so they cannot crowd the control bus.
    resource_rx: Option<broadcast::Receiver<gosub_engine::events::ResourceUpdate>>,
}

impl<C: BeaconConfig> BrowserEngine<C> {
    /// Build and start the engine. Must be called with `rt` as the active tokio runtime
    /// for engine tasks to spawn correctly.
    ///
    /// `backend` is the frontend's rasterizer; the engine takes ownership of it.
    ///
    /// A `private` engine backs a private-browsing window: cookies, local storage and
    /// session storage live in memory only, and the engine records no visited history.
    /// Settings are still read from (and written to) the shared store, and the persistent
    /// bookmarks/history remain readable for the UI (bookmarks bar, URL completion) —
    /// matching what mainstream browsers do in private mode.
    pub fn new(rt: &Runtime, private: bool, backend: Arc<C::RenderBackend>) -> anyhow::Result<Self> {
        let _guard = rt.enter();

        let (tx_redraw, rx_redraw) = mpsc::unbounded_channel::<()>();
        let compositor = Arc::new(DefaultCompositor::new(move || {
            let _ = tx_redraw.send(());
        }));

        let mut engine = GosubEngine::<C>::new(None, backend, compositor.clone());

        let data_dir = crate::paths::data_dir();
        let isolated = crate::cli::Cli::global().isolated();

        // Beacon's own settings, merged into the engine's store under the `useragent`
        // namespace the client schema already owns. Registering here rather than editing the
        // engine's useragent-settings.json keeps a shell-only preference in the shell, which
        // is the direction that schema is headed anyway ("intended to move to a dedicated
        // client crate, which would then merge it in itself").
        //
        // MUST come before `set_storage`: merged settings are an in-memory snapshot, and
        // `set_storage` loads stored values for the keys it knows about at attach time. Merge
        // afterwards and the key exists but its persisted value is never read back, so the
        // setting silently always reports its default.
        engine.settings().merge(&beacon_settings(), "useragent");

        // Persist settings overrides (edited via gosub://config) across runs. Attached
        // before anything reads or writes settings, so stored values win over defaults.
        // Falls back to the in-memory store if the database cannot be opened.
        let settings_db = data_dir.join("settings.db").to_string_lossy().into_owned();
        match gosub_engine::config_storage::SqliteStorageAdapter::try_from(&settings_db) {
            Ok(storage) => engine.settings().set_storage(Box::new(storage)),
            Err(e) => log::warn!("settings database {settings_db} unavailable, settings will not persist: {e:?}"),
        }

        // Beacon-branded versions of the engine's built-in gosub://home and gosub://help, and
        // of its page for unknown names. Everything else (blank, version, history, config
        // dump) is the engine's own; gosub://config additionally gets a shell-rendered editor.
        // The page carries the build's own version, so a preview build says which one it is
        // rather than just that it is one. Each declines while BEACON_PAGES is off, which hands
        // the request to the engine's own page; it is read per request, so flipping it in
        // gosub://config shows on the next load.
        engine.internal_pages().register(
            "home",
            beacon_page(include_str!("../resources/home.html").replace("{{VERSION}}", env!("CARGO_PKG_VERSION"))),
        );
        engine
            .internal_pages()
            .register("help", beacon_page(include_str!("../resources/help.html").to_string()));
        engine.internal_pages().set_not_found(Arc::new(|req, known| {
            req.settings
                .get_bool(BEACON_PAGES)
                .then(|| gosub_engine::internal_pages::PageResponse::html(crate::not_found_page::build(req.name, req.url.as_str(), known)))
        }));

        // Identify as Beacon on the wire; the engine alone would send only its Gosub
        // token. Only seeded when nothing is stored, so a user-customized UA survives
        // restarts. Must land before start(), which reads the network settings once.
        if engine.settings().get_string("net.user_agent").is_empty() {
            let ua = gosub_engine::net::default_user_agent(Some(concat!("Beacon/", env!("CARGO_PKG_VERSION"))));
            if let Err(e) = engine.settings().set("net.user_agent", gosub_engine::Setting::String(ua)) {
                log::warn!("failed to set net.user_agent: {e:?}");
            }
        }

        // The switch follows the command line (and the build: an `isolation` build is on
        // unless `--single-process`), so a plain run stays the single-process engine it
        // always was. Set for this run rather than stored: the engine's own defaults (and
        // the user's `security.process_isolation` choice in gosub://config) only take effect
        // once this gating comes off. Must land before start(), which reads the process
        // settings once.
        // The renderer tier needs more than the switch: a forked rasterizer and a font
        // system the engine can confine fully, both of which only the GTK frontend's
        // `isolation` build feature provides. Without them the engine says so at start
        // and keeps page rendering in-process; the switch still gets the other processes.
        engine
            .set_process_isolation_for_this_run(isolated)
            .map_err(|e| anyhow::anyhow!("security.process_isolation: {e:?}"))?;

        // start() hands back the engine main-loop future; it only runs once spawned.
        let engine_loop = engine.start().map_err(|e| anyhow::anyhow!("engine start: {e:?}"))?;
        tokio::spawn(engine_loop);

        // Subscribe before creating the zone: `create_zone` emits `ZoneCreated` on the
        // event channel, which errors out ("channel closed") if there is no live receiver.
        let event_rx = engine.subscribe_events();
        let resource_rx = engine.subscribe_resource_events();

        let zone_cfg = ZoneConfig::builder()
            .do_not_track(true)
            // Without this the engine sends no Accept-Language at all.
            .accept_languages(crate::locale::accept_language())
            .build()
            .map_err(|e| anyhow::anyhow!("ZoneConfig: {e:?}"))?;

        let local_db = data_dir.join("local-storage.db").to_string_lossy().into_owned();

        // Bookmarks + visited history. The engine records visits; Beacon queries the same
        // handle directly (star button, bookmarks bar, URL-bar completion).
        let places: gosub_engine::places::PlacesHandle =
            Arc::new(SqlitePlaces::new(data_dir.join("places.db")).map_err(|e| anyhow::anyhow!("places: {e:?}"))?);
        if !private && places.bookmarks().is_empty() {
            for (url, title) in [
                ("https://gosub.io", "Gosub"),
                ("https://github.com/gosub-io", "GitHub"),
                ("https://news.ycombinator.com", "Hacker News"),
            ] {
                places.add_bookmark(url, title);
            }
        }

        // gosub://bookmarks: rendered from the live store on every request.
        engine.internal_pages().register("bookmarks", {
            let places = places.clone();
            Arc::new(move |_req: &gosub_engine::internal_pages::PageRequest<'_>| {
                let mut body = String::from(
                    "<h1>Bookmarks</h1><p class=\"sub\">The star button in the toolbar adds and removes bookmarks.</p><table>",
                );
                for b in places.bookmarks() {
                    let url = html_escape(&b.url);
                    body.push_str(&format!(
                        "<tr><td><a href=\"{url}\">{}</a></td><td class=\"muted\"><code>{url}</code></td></tr>",
                        html_escape(&b.title)
                    ));
                }
                body.push_str("</table>");
                Some(gosub_engine::internal_pages::PageResponse::html(format!(
                    "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>Bookmarks</title><style>\
                     body{{margin:0;padding:32px 40px;font-family:sans-serif;font-size:14px}}\
                     h1{{font-size:24px;margin:0 0 4px 0}} .sub{{color:#5c6675;margin:0 0 20px 0}}\
                     table{{border-collapse:collapse}} td{{padding:4px 14px 4px 0}}\
                     a{{color:#1d5fd1}} .muted{{color:#8a94a6}} code{{font-family:inherit;letter-spacing:0.03em;font-size:12px}}\
                     </style></head><body>{body}</body></html>"
                )))
            })
        });

        let zone_services = if private {
            // Everything in memory, nothing recorded: gone when the window closes.
            ZoneServices {
                storage: Arc::new(StorageService::new(
                    Arc::new(gosub_engine::storage::InMemoryLocalStore::new()),
                    Arc::new(InMemorySessionStore::new()),
                )),
                cookie_store: None,
                cookie_jar: Some(gosub_engine::cookies::DefaultCookieJar::new().into()),
                partition_policy: PartitionPolicy::None,
                places: None,
            }
        } else {
            let cookie_store: gosub_engine::cookies::CookieStoreHandle = SqliteCookieStore::new(data_dir.join("cookies.db"))
                .map_err(|e| anyhow::anyhow!("cookie store: {e:?}"))?
                .into();
            // Isolated: localStorage lives in file areas served by the engine's storage
            // process, the one store kind that process can serve; the SQLite store stays
            // in this process. The two builds therefore keep separate localStorage for the
            // same profile, which is accepted: the isolated build is a different mode.
            // The storage process is Linux only, like the rest of the tier; elsewhere the
            // isolated mode keeps the SQLite store.
            #[cfg(target_os = "linux")]
            let local: Arc<dyn gosub_engine::storage::LocalStore> = if isolated {
                let dir = data_dir.join("local-storage");
                Arc::new(
                    gosub_engine::storage::ServiceLocalStore::new(&dir)
                        .map_err(|e| anyhow::anyhow!("local store service in {}: {e:?}", dir.display()))?,
                )
            } else {
                Arc::new(SqliteLocalStore::new(&local_db).map_err(|e| anyhow::anyhow!("local store: {e:?}"))?)
            };
            #[cfg(not(target_os = "linux"))]
            let local: Arc<dyn gosub_engine::storage::LocalStore> =
                Arc::new(SqliteLocalStore::new(&local_db).map_err(|e| anyhow::anyhow!("local store: {e:?}"))?);
            ZoneServices {
                storage: Arc::new(StorageService::new(local, Arc::new(InMemorySessionStore::new()))),
                cookie_store: Some(cookie_store),
                cookie_jar: None,
                partition_policy: PartitionPolicy::None,
                places: Some(places.clone()),
            }
        };

        let zone = engine
            .zone_builder()
            .config(zone_cfg)
            .id(ZoneId::from(DEFAULT_ZONE))
            .services(zone_services)
            .create()
            .map_err(|e| anyhow::anyhow!("create_zone: {e:?}"))?;

        Ok(Self {
            places,
            engine,
            zone,
            compositor,
            redraw_rx: Some(rx_redraw),
            event_rx: Some(event_rx),
            resource_rx: Some(resource_rx),
        })
    }

    /// Bookmarks + visited history (star button, bookmarks bar, URL completion).
    pub fn places(&self) -> gosub_engine::places::PlacesHandle {
        self.places.clone()
    }

    /// The engine's settings store (backs the `gosub://config` page).
    pub fn settings(&self) -> &gosub_engine::Config {
        self.engine.settings()
    }

    /// Take the engine event stream (navigation, redraw, hover, …). Only the first
    /// caller receives the receiver that was subscribed before zone creation.
    pub fn take_event_rx(&mut self) -> Option<broadcast::Receiver<EngineEvent>> {
        self.event_rx.take()
    }

    /// Take the resource event stream (per-request fetch detail for the developer panel).
    /// Only the first caller receives it.
    pub fn take_resource_rx(&mut self) -> Option<broadcast::Receiver<gosub_engine::events::ResourceUpdate>> {
        self.resource_rx.take()
    }

    /// Take the redraw notification receiver (drains compositor frame notifications).
    /// Only the first caller receives it.
    pub fn take_redraw_rx(&mut self) -> Option<mpsc::UnboundedReceiver<()>> {
        self.redraw_rx.take()
    }

    /// Create a fresh engine tab in the default zone. Blocks on the runtime.
    ///
    /// `viewport` is the initial size in CSS px, from `viewport_for_new_tab()`: a hidden
    /// `GtkStack` page is never allocated, so its GLArea's resize handler (the only other
    /// `SetViewport` source) does not fire until the tab is first shown. Without an initial
    /// viewport, background tabs lay out and rasterize at the wrong size and must fully
    /// re-render on switch. `None` is safe — the engine applies its own non-zero fallback —
    /// but prefer a real size, since a viewport that differs when the tab is first shown
    /// costs a full cache drop and re-layout.
    pub fn create_tab(&mut self, rt: &Runtime, title: &str, viewport: Option<(u32, u32)>) -> anyhow::Result<TabHandle> {
        let mut builder = self.zone.tab_builder().title(title);
        // Falls back to the first GTK resize when the window has no allocation yet.
        if let Some((w, h)) = viewport {
            builder = builder.viewport(gosub_render_pipeline::render::Viewport::new(0, 0, w, h));
        }

        let tab = rt.block_on(builder.create()).map_err(|e| anyhow::anyhow!("create_tab: {e:?}"))?;
        Ok(tab)
    }
}

/// Beacon's own versions of the engine's home, help and not-found pages, or the engine's.
const BEACON_PAGES: &str = "useragent.general.beacon_pages";

/// Beacon's settings, merged into the engine's store under `useragent`.
fn beacon_settings() -> gosub_engine::Config {
    gosub_engine::Config::new(vec![
        gosub_engine::SettingInfo {
            key: "general.homepage".to_string(),
            description: "Page the Home button navigates to.".to_string(),
            default: gosub_engine::Setting::String("gosub://home".to_string()),
            constraint: None,
        },
        gosub_engine::SettingInfo {
            key: "general.beacon_pages".to_string(),
            description: "Show Beacon's own gosub://home, gosub://help and page-not-found; off shows the engine's built-in ones."
                .to_string(),
            default: gosub_engine::Setting::Bool(true),
            constraint: None,
        },
    ])
}

/// A page that is Beacon's `html` while [`BEACON_PAGES`] is on, and declines otherwise.
fn beacon_page(html: String) -> gosub_engine::internal_pages::PageProvider {
    Arc::new(move |req: &gosub_engine::internal_pages::PageRequest<'_>| {
        req.settings
            .get_bool(BEACON_PAGES)
            .then(|| gosub_engine::internal_pages::PageResponse::html(html.clone()))
    })
}

/// Minimal HTML escaping for the bookmarks page.
fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use gosub_engine::internal_pages::{PageRequest, TabView};

    fn serve(provider: &gosub_engine::internal_pages::PageProvider, settings: &gosub_engine::Config) -> Option<String> {
        let url = url::Url::parse("gosub://home").unwrap();
        let req = PageRequest {
            name: "home",
            url: &url,
            settings,
            tab: &TabView::default(),
        };
        provider(&req).map(|page| page.html)
    }

    #[test]
    fn beacons_pages_give_way_to_the_engines_when_switched_off() {
        let settings = gosub_engine::Config::new(Vec::new());
        settings.merge(&beacon_settings(), "useragent");
        let home = beacon_page("BEACON".to_string());

        assert_eq!(serve(&home, &settings).as_deref(), Some("BEACON"), "on by default");
        settings.set_transient(BEACON_PAGES, gosub_engine::Setting::Bool(false)).unwrap();
        assert_eq!(serve(&home, &settings), None, "declines, so the engine's page answers");
    }
}
