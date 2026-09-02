/*
 * Gosub Beacon — C ABI over beacon-core.
 *
 * A chrome written in Swift, C# or anything else that speaks C drives the browser through
 * this header. The browser itself — tabs, navigation, history, downloads — lives in Rust
 * and is shared by every shell.
 *
 * Four rules the whole boundary rests on:
 *
 *   1. The shell keeps no state. It asks: beacon_tab_count, beacon_tab_at,
 *      beacon_tab_title. Two lists that can disagree is a bug this project has already
 *      had; there is one owner and you ask it every time.
 *
 *   2. Events are pulled, never pushed. Call beacon_poll_events from your run loop. There
 *      are no callbacks, because a callback would fire on whichever Rust thread noticed
 *      and both AppKit and WinUI insist on the UI thread.
 *
 *   3. Call everything from one thread — your UI thread. The engine's own work happens on
 *      background threads it manages itself and never touches these types.
 *
 *   4. Strings returned to you are yours to free with beacon_string_free. Strings inside
 *      a BeaconEvent are borrowed, and stop being valid at the next beacon_poll_events.
 *
 * This header is written by hand rather than generated, so it can carry that reasoning.
 * examples/smoke.c compiles against it, which is what stops it drifting from the Rust.
 */

#ifndef BEACON_H
#define BEACON_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* An open browser. Create with beacon_new, destroy with beacon_free. */
typedef struct BeaconBrowser BeaconBrowser;

/* A tab. 0 is never a valid tab, so it doubles as "none". Stays valid until the tab is
 * closed; do not assume anything about the numbering. */
typedef uint64_t BeaconTabId;

typedef struct {
    /* Profile directory (cookies, history, settings). NULL for the platform default:
     * ~/.local/share/gosub-beacon on Linux, ~/Library/Application Support on macOS. */
    const char *user_data_dir;
    /* Ephemeral cookies and storage, nothing written to history. */
    bool private_mode;
} BeaconConfig;

typedef enum {
    BEACON_REDRAW = 0,             /* a new frame is ready; re-acquire and repaint       */
    BEACON_TABS_CHANGED = 1,       /* the tab strip needs rebuilding                     */
    BEACON_ACTIVE_TAB_CHANGED = 2, /* `tab` is now frontmost                             */
    BEACON_TITLE_CHANGED = 3,      /* `text` is the new title                            */
    BEACON_URL_CHANGED = 4,        /* `text` is the new URL                              */
    BEACON_LOADING_CHANGED = 5,    /* `number` is 1 while loading, 0 when done           */
    BEACON_PROGRESS = 6,           /* `number` is 0..1, or -1 to clear the indicator     */
    BEACON_FAVICON_CHANGED = 7,    /* re-read the icon for `tab`                         */
    BEACON_NAV_STATE_CHANGED = 8,  /* back/forward availability changed                  */
    BEACON_HOVER_URL = 9,          /* `text` is the link under the pointer, or NULL      */
    BEACON_CURSOR_CHANGED = 10,    /* `number`: 0 default, 1 pointer, 2 text             */
    BEACON_DOWNLOAD_OFFERED = 11,  /* `text` suggested filename, `number` the offer id   */
    BEACON_TAB_CRASHED = 12,       /* `text` is the reason; the tab is still in the strip */
    BEACON_LOG = 13,               /* `text` is worth showing a developer                */
    BEACON_DOWNLOAD_CHANGED = 14   /* `number` is the download id; re-read its progress  */
} BeaconEventKind;

typedef struct {
    BeaconEventKind kind;
    /* The tab this concerns, or 0 if it is not about one tab. */
    BeaconTabId tab;
    /* Borrowed until the next beacon_poll_events. NULL when the event carries no text.
     * Copy it if you need to keep it. */
    const char *text;
    double number;
} BeaconEvent;

typedef struct {
    /* BGRA, premultiplied alpha, `stride` bytes per row. Borrowed until
     * beacon_release_frame — do not free, do not keep. */
    const uint8_t *pixels;
    uint32_t width;  /* in device pixels, i.e. already multiplied by dpr */
    uint32_t height;
    uint32_t stride;
    uint32_t dpr; /* device pixels per CSS pixel */
} BeaconFrame;

typedef enum { BEACON_BUTTON_LEFT = 0, BEACON_BUTTON_MIDDLE = 1, BEACON_BUTTON_RIGHT = 2 } BeaconButton;

/* ── lifecycle ─────────────────────────────────────────────────────────────── */

/* NULL if the engine could not start. `config` may be NULL for defaults. */
BeaconBrowser *beacon_new(const BeaconConfig *config);
void beacon_free(BeaconBrowser *browser);
/* Free a string this library returned. NULL is fine. */
void beacon_string_free(char *s);

/* Whether this browser is a private session, as asked for in BeaconConfig. Cookies, local
 * storage and session storage are in memory only and no visited history is recorded;
 * bookmarks and settings are still the shared persistent ones, as in every mainstream
 * browser. Ask rather than remember which handle is which. */
bool beacon_is_private(BeaconBrowser *browser);

/* ── tabs ──────────────────────────────────────────────────────────────────── */

/* Open a tab and start loading. Returns 0 if the URL could not be parsed. Accepts what a
 * user would type: "example.com", "gosub://home", "/etc/hosts". */
BeaconTabId beacon_open_tab(BeaconBrowser *browser, const char *url);
/* Refuses to close the last tab, as the GTK shell does. */
void beacon_close_tab(BeaconBrowser *browser, BeaconTabId tab);
/* Also suspends drawing in the tab being left and resumes it in this one, so background
 * tabs are not still painting at 30fps. */
void beacon_activate_tab(BeaconBrowser *browser, BeaconTabId tab);
/* Bring back the most recently closed tab, at the position it was closed from. 0 when
 * there is nothing to reopen. */
BeaconTabId beacon_reopen_closed_tab(BeaconBrowser *browser);

size_t beacon_tab_count(BeaconBrowser *browser);
/* In strip order; 0 if `index` is out of range. */
BeaconTabId beacon_tab_at(BeaconBrowser *browser, size_t index);
BeaconTabId beacon_active_tab(BeaconBrowser *browser);

/* Free the result with beacon_string_free. NULL if the tab is gone. */
char *beacon_tab_title(BeaconBrowser *browser, BeaconTabId tab);
char *beacon_tab_url(BeaconBrowser *browser, BeaconTabId tab);

bool beacon_tab_is_loading(BeaconBrowser *browser, BeaconTabId tab);
bool beacon_tab_can_go_back(BeaconBrowser *browser, BeaconTabId tab);
bool beacon_tab_can_go_forward(BeaconBrowser *browser, BeaconTabId tab);

/* Load progress 0..1, or -1 when nothing is loading or the server never sent a length —
 * show an indeterminate indicator for -1 rather than a full bar. */
double beacon_tab_progress(BeaconBrowser *browser, BeaconTabId tab);

/* The favicon exactly as the site served it, usually PNG or ICO; you decode it. NULL and
 * *out_len = 0 when the tab has none. Borrowed until the NEXT call to this function, like
 * event strings — copy it into an NSImage and forget the pointer. */
const uint8_t *beacon_tab_favicon(BeaconBrowser *browser, BeaconTabId tab, size_t *out_len);

bool beacon_tab_is_pinned(BeaconBrowser *browser, BeaconTabId tab);
/* Pinned tabs sit at the left of the strip and resist closing. That rule lives in the
 * browser, not in your shell. */
void beacon_set_tab_pinned(BeaconBrowser *browser, BeaconTabId tab, bool pinned);
/* What a drag in the tab bar means. `index` is a strip position. */
void beacon_move_tab(BeaconBrowser *browser, BeaconTabId tab, size_t index);

/* ── commands ──────────────────────────────────────────────────────────────── */

void beacon_navigate(BeaconBrowser *browser, BeaconTabId tab, const char *url);
/* These act on the active tab: the browser knows which that is, so the shell need not. */
void beacon_back(BeaconBrowser *browser);
void beacon_forward(BeaconBrowser *browser);
void beacon_reload(BeaconBrowser *browser, bool ignore_cache);
void beacon_stop(BeaconBrowser *browser);

/* ── input ─────────────────────────────────────────────────────────────────── */

/* Page area in CSS pixels, plus device pixels per CSS pixel (NSView.backingScaleFactor,
 * or 1.0 if you have no idea). Send this whenever your view resizes or moves between
 * displays; nothing renders until the engine knows how big the page is.
 *
 * Get `scale` wrong and text looks blurry: the page is rasterized at 1x and stretched onto
 * a 2x surface. It is not the font rendering. */
void beacon_set_viewport(BeaconBrowser *browser, BeaconTabId tab, uint32_t width, uint32_t height, float scale);
void beacon_mouse_move(BeaconBrowser *browser, BeaconTabId tab, float x, float y);
void beacon_mouse_down(BeaconBrowser *browser, BeaconTabId tab, float x, float y, BeaconButton button);
void beacon_mouse_up(BeaconBrowser *browser, BeaconTabId tab, float x, float y, BeaconButton button);
void beacon_scroll(BeaconBrowser *browser, BeaconTabId tab, float delta_x, float delta_y);

/* Page zoom, 1.0 = 100%, clamped to 0.25..5.0. Keep sending the view's UNZOOMED size to
 * beacon_set_viewport; zoom is applied on this side, so the two never drift apart. */
void beacon_set_zoom(BeaconBrowser *browser, BeaconTabId tab, float zoom);
float beacon_zoom(BeaconBrowser *browser, BeaconTabId tab);

/* Modifier bits for the key functions. BEACON_MOD_META is Command on macOS. */
#define BEACON_MOD_SHIFT 1u
#define BEACON_MOD_CONTROL 2u
#define BEACON_MOD_ALT 4u
#define BEACON_MOD_META 8u

/* `key` and `code` are the web's own names: KeyboardEvent.key ("a", "Enter", "ArrowLeft")
 * and KeyboardEvent.code, the physical key ("KeyA"). Your shell does that mapping because
 * only your shell knows the keyboard layout — on AZERTY the "a" key is "KeyQ", and nothing
 * on this side can work that out. `code` may be NULL.
 *
 * Send beacon_text_input for what was actually typed. On macOS that is what
 * NSTextInputClient hands you, and it is the only route by which dead keys, CJK input
 * methods and emoji reach the page intact — do not synthesise it from key names. */
void beacon_key_down(BeaconBrowser *browser, BeaconTabId tab, const char *key, const char *code, uint32_t modifiers);
void beacon_key_up(BeaconBrowser *browser, BeaconTabId tab, const char *key, const char *code, uint32_t modifiers);
void beacon_text_input(BeaconBrowser *browser, BeaconTabId tab, const char *text);

/* ── bookmarks ─────────────────────────────────────────────────────────────── */

bool beacon_tab_is_bookmarked(BeaconBrowser *browser, BeaconTabId tab);
/* Returns the state it ended in. Internal gosub:// pages are never bookmarkable and
 * always return false. */
bool beacon_toggle_bookmark(BeaconBrowser *browser, BeaconTabId tab);

size_t beacon_bookmark_count(BeaconBrowser *browser);
/* Free with beacon_string_free; NULL when `index` is out of range. */
char *beacon_bookmark_url(BeaconBrowser *browser, size_t index);
char *beacon_bookmark_title(BeaconBrowser *browser, size_t index);

/* ── history ───────────────────────────────────────────────────────────────── */

/* Search visited pages, newest and most-visited first, and return the number of matches
 * (capped at `limit`). Read the rows with the accessors below; they stay valid until the
 * next search, which is what lets an address bar re-query on every keystroke.
 *
 * An empty query matches nothing rather than everything — a suggestion list for "" is a
 * history browser, not an autocomplete. */
size_t beacon_history_search(BeaconBrowser *browser, const char *query, size_t limit);
/* Free with beacon_string_free; NULL when out of range. */
char *beacon_history_url(BeaconBrowser *browser, size_t index);
char *beacon_history_title(BeaconBrowser *browser, size_t index);
uint64_t beacon_history_visit_count(BeaconBrowser *browser, size_t index);

/* ── downloads ─────────────────────────────────────────────────────────────── */

typedef enum {
    BEACON_DOWNLOAD_RUNNING = 0,
    BEACON_DOWNLOAD_FINISHED = 1,
    BEACON_DOWNLOAD_FAILED = 2
} BeaconDownloadState;

/* A BEACON_DOWNLOAD_OFFERED event carries a suggested filename in `text` and an offer id
 * in `number`. Answer it with accept or reject — the id is what keeps the answer attached
 * to the right offer when a save panel is up and a second download arrives.
 *
 * You choose the path, because where a file belongs is a platform question: on macOS run
 * an NSSavePanel. beacon_download_accept returns a download id to track it with, or 0. */
char *beacon_download_offer_url(BeaconBrowser *browser, uint64_t offer);
uint64_t beacon_download_accept(BeaconBrowser *browser, uint64_t offer, const char *path);
void beacon_download_reject(BeaconBrowser *browser, uint64_t offer);

size_t beacon_download_count(BeaconBrowser *browser);
uint64_t beacon_download_at(BeaconBrowser *browser, size_t index);
char *beacon_download_filename(BeaconBrowser *browser, uint64_t id);
char *beacon_download_path(BeaconBrowser *browser, uint64_t id);
/* 0..1, or -1 when the server sent no length — show bytes received instead of a bar. */
double beacon_download_progress(BeaconBrowser *browser, uint64_t id);
uint64_t beacon_download_received(BeaconBrowser *browser, uint64_t id);
BeaconDownloadState beacon_download_state(BeaconBrowser *browser, uint64_t id);
/* Hand a finished download to the desktop's default application. */
void beacon_download_open(BeaconBrowser *browser, uint64_t id);

/* ── events ────────────────────────────────────────────────────────────────── */

/* Writes up to `max` events into `out` and returns how many. Call it until it returns 0,
 * from your run loop. Text in the events is invalidated by the next call. */
size_t beacon_poll_events(BeaconBrowser *browser, BeaconEvent *out, size_t max);

/* ── the page, two ways ────────────────────────────────────────────────────────
 *
 * A native chrome should attach a view: the page is drawn straight into it on the GPU,
 * with no copy. Everything else -- tests, screenshots, thumbnails -- takes the frame,
 * which is pixels in memory and works everywhere.
 */

/* Draw this tab into a view you own and lay out yourself: an NSView* on macOS, an HWND on
 * Windows. Beacon fills it; you position it among your tab bar and toolbar like any other
 * subview. Sizes are in device pixels.
 *
 * Returns false on platforms without a GPU path, or if the view cannot be wrapped -- fall
 * back to beacon_acquire_frame if it does.
 *
 * The view must outlive the attachment: call beacon_detach_view before destroying it. */
bool beacon_attach_view(BeaconBrowser *browser, BeaconTabId tab, void *view, uint32_t width, uint32_t height);
void beacon_detach_view(BeaconBrowser *browser, BeaconTabId tab);
/* Tell Beacon the view resized, in device pixels. */
void beacon_resize_view(BeaconBrowser *browser, BeaconTabId tab, uint32_t width, uint32_t height);
/* Draw the latest frame into the attached view. Call on BEACON_REDRAW, from your own draw
 * cycle. False if no view is attached or nothing has rendered yet. */
bool beacon_draw_view(BeaconBrowser *browser, BeaconTabId tab);

/* Lends you the tab's latest frame as pixels. false when nothing has rendered yet, which is
 * normal for the first moments after opening a tab. Pair every true with a release.
 *
 * Where the page is rendered on the GPU this copies it back off the card, which is slow on
 * purpose: if you are drawing a window, attach a view instead. */
bool beacon_acquire_frame(BeaconBrowser *browser, BeaconTabId tab, BeaconFrame *out);
void beacon_release_frame(BeaconBrowser *browser, BeaconTabId tab);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* BEACON_H */
