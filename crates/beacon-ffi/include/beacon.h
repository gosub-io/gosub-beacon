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
    BEACON_DOWNLOAD_OFFERED = 11,  /* `text` is a suggested filename; ask the user        */
    BEACON_TAB_CRASHED = 12,       /* `text` is the reason; the tab is still in the strip */
    BEACON_LOG = 13                /* `text` is worth showing a developer                */
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

/* ── tabs ──────────────────────────────────────────────────────────────────── */

/* Open a tab and start loading. Returns 0 if the URL could not be parsed. Accepts what a
 * user would type: "example.com", "gosub://home", "/etc/hosts". */
BeaconTabId beacon_open_tab(BeaconBrowser *browser, const char *url);
/* Refuses to close the last tab, as the GTK shell does. */
void beacon_close_tab(BeaconBrowser *browser, BeaconTabId tab);
void beacon_activate_tab(BeaconBrowser *browser, BeaconTabId tab);

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

/* ── commands ──────────────────────────────────────────────────────────────── */

void beacon_navigate(BeaconBrowser *browser, BeaconTabId tab, const char *url);
/* These act on the active tab: the browser knows which that is, so the shell need not. */
void beacon_back(BeaconBrowser *browser);
void beacon_forward(BeaconBrowser *browser);
void beacon_reload(BeaconBrowser *browser, bool ignore_cache);
void beacon_stop(BeaconBrowser *browser);

/* ── input ─────────────────────────────────────────────────────────────────── */

/* Page area in CSS pixels. Send this whenever your view resizes; nothing renders until
 * the engine knows how big the page is. */
void beacon_set_viewport(BeaconBrowser *browser, BeaconTabId tab, uint32_t width, uint32_t height);
void beacon_mouse_move(BeaconBrowser *browser, BeaconTabId tab, float x, float y);
void beacon_mouse_down(BeaconBrowser *browser, BeaconTabId tab, float x, float y, BeaconButton button);
void beacon_scroll(BeaconBrowser *browser, BeaconTabId tab, float delta_x, float delta_y);

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
