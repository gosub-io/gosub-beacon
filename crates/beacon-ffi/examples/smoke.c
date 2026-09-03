/*
 * The first consumer of the Beacon C ABI — deliberately in C, and deliberately headless.
 *
 * If the boundary is wrong, finding out here costs a compile. Finding out through Xcode
 * costs an afternoon. So before anyone writes Swift, this drives the ABI the way a real
 * shell would and checks what came back:
 *
 *   1. a page loads and composites
 *   2. tabs open, close, and the last one refuses to close
 *   3. back and forward walk real session history
 *   4. scrolling changes what is drawn
 *   5. zoom, pinning, reordering and reopening behave
 *   6. bookmarks round-trip, and downloads answer the right offer
 *
 *   make -C crates/beacon-ffi run
 *
 * Test pages are written to /tmp and loaded over file://, so the run is deterministic and
 * needs no network. Exits non-zero if any check fails.
 */

/* nanosleep is POSIX, and -std=c11 alone does not expose it on glibc. Not on Darwin
 * though: there this macro sets __DARWIN_C_LEVEL and *narrows* what the headers declare,
 * so asking for it can hide more than it reveals. macOS exposes nanosleep by default. */
#if !defined(__APPLE__)
#define _POSIX_C_SOURCE 199309L
#endif

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "beacon.h"

static int failures = 0;

#define CHECK(cond, ...)                          \
    do {                                          \
        if (cond) {                               \
            printf("  ok    ");                   \
        } else {                                  \
            printf("  FAIL  ");                   \
            failures++;                           \
        }                                         \
        printf(__VA_ARGS__);                      \
        printf("\n");                             \
    } while (0)

static void sleep_ms(long ms) {
    struct timespec t = {.tv_sec = ms / 1000, .tv_nsec = (ms % 1000) * 1000 * 1000};
    nanosleep(&t, NULL);
}

/* Drain the event queue the way a shell's run loop would. Events are dropped on the floor
 * here; the checks below read state back through the query functions instead, which is
 * also the discipline the ABI asks of a real shell. */
static void pump(BeaconBrowser *browser) {
    BeaconEvent events[64];
    while (beacon_poll_events(browser, events, 64) > 0) {
        /* keep draining */
    }
}

/* Pump until the tab stops loading, or the budget runs out. */
static int settle(BeaconBrowser *browser, BeaconTabId tab, int budget_ms) {
    for (int waited = 0; waited < budget_ms; waited += 50) {
        pump(browser);
        if (!beacon_tab_is_loading(browser, tab)) {
            /* One more slice: the frame usually lands just after loading clears. */
            sleep_ms(100);
            pump(browser);
            return 1;
        }
        sleep_ms(50);
    }
    return 0;
}

/* Pump until a frame exists, or the budget runs out. Loading clearing and the first
 * composite are separate events, and on a local file they can be far enough apart to
 * matter. */
static int wait_for_frame(BeaconBrowser *browser, BeaconTabId tab, int budget_ms);

/* FNV-1a over the frame, so two renders can be compared without keeping both. Returns 0
 * when nothing has been composited yet. */
static unsigned long long frame_digest(BeaconBrowser *browser, BeaconTabId tab) {
    BeaconFrame frame;
    if (!beacon_acquire_frame(browser, tab, &frame)) {
        return 0;
    }
    unsigned long long hash = 1469598103934665603ULL;
    for (uint32_t y = 0; y < frame.height; y++) {
        const uint8_t *row = frame.pixels + (size_t)y * frame.stride;
        for (uint32_t x = 0; x < frame.width * 4; x++) {
            hash = (hash ^ row[x]) * 1099511628211ULL;
        }
    }
    beacon_release_frame(browser, tab);
    return hash;
}

/* BGRA premultiplied -> binary PPM, so a run can be eyeballed with any image viewer. */
static int write_ppm(const char *path, BeaconBrowser *browser, BeaconTabId tab) {
    BeaconFrame frame;
    if (!beacon_acquire_frame(browser, tab, &frame)) {
        return 0;
    }
    FILE *f = fopen(path, "wb");
    if (!f) {
        beacon_release_frame(browser, tab);
        return 0;
    }
    fprintf(f, "P6\n%u %u\n255\n", frame.width, frame.height);
    for (uint32_t y = 0; y < frame.height; y++) {
        const uint8_t *row = frame.pixels + (size_t)y * frame.stride;
        for (uint32_t x = 0; x < frame.width; x++) {
            const uint8_t *px = row + (size_t)x * 4;
            fputc(px[2], f); /* BGRA in memory, RGB on disk */
            fputc(px[1], f);
            fputc(px[0], f);
        }
    }
    fclose(f);
    beacon_release_frame(browser, tab);
    return 1;
}

/* Deterministic local pages, so the run does not depend on the network or on anyone
 * else's markup staying still. */
static int write_page(const char *path, const char *body) {
    FILE *f = fopen(path, "w");
    if (!f) {
        return 0;
    }
    fprintf(f, "<!doctype html><html><head><meta charset=\"utf-8\"><title>%s</title></head><body>%s</body></html>\n", path, body);
    fclose(f);
    return 1;
}

/* A page tall enough that scrolling has somewhere to go, striped so each screenful looks
 * different from the last. */
static int write_tall_page(const char *path) {
    FILE *f = fopen(path, "w");
    if (!f) {
        return 0;
    }
    fprintf(f, "<!doctype html><html><head><meta charset=\"utf-8\"><title>tall</title></head><body style=\"margin:0\">");
    for (int i = 0; i < 40; i++) {
        fprintf(f, "<div style=\"height:200px;background:%s\"><h1>block %d</h1></div>", (i % 2) ? "#3355aa" : "#ddeeff", i);
    }
    fprintf(f, "</body></html>\n");
    fclose(f);
    return 1;
}

static int wait_for_frame(BeaconBrowser *browser, BeaconTabId tab, int budget_ms) {
    for (int waited = 0; waited < budget_ms; waited += 100) {
        pump(browser);
        if (frame_digest(browser, tab) != 0) {
            return 1;
        }
        sleep_ms(100);
    }
    return 0;
}

int main(void) {
    const char *page_a = "/tmp/beacon-ffi-a.html";
    const char *page_b = "/tmp/beacon-ffi-b.html";
    const char *page_tall = "/tmp/beacon-ffi-tall.html";
    char url_a[256], url_b[256], url_tall[256];

    if (!write_page(page_a, "<h1>Page A</h1><p>first</p>") || !write_page(page_b, "<h1>Page B</h1><p>second</p>") ||
        !write_tall_page(page_tall)) {
        fprintf(stderr, "could not write test pages to /tmp\n");
        return 1;
    }
    snprintf(url_a, sizeof url_a, "file://%s", page_a);
    snprintf(url_b, sizeof url_b, "file://%s", page_b);
    snprintf(url_tall, sizeof url_tall, "file://%s", page_tall);

    BeaconConfig config = {.user_data_dir = "/tmp/beacon-ffi-profile", .private_mode = false};
    BeaconBrowser *browser = beacon_new(&config);
    if (!browser) {
        fprintf(stderr, "beacon_new failed\n");
        return 1;
    }

    CHECK(!beacon_is_private(browser), "a browser made without private_mode is not private");

    /* ── 1. a page loads and composites ────────────────────────────────── */
    printf("\nloading and rendering\n");
    BeaconTabId tab = beacon_open_tab(browser, url_a);
    CHECK(tab != 0, "opening a tab returns a handle");
    beacon_set_viewport(browser, tab, 1024, 768, 1.0f);
    CHECK(settle(browser, tab, 15000), "the page finishes loading");

    char *url = beacon_tab_url(browser, tab);
    CHECK(url && strstr(url, "beacon-ffi-a.html") != NULL, "the tab reports the URL it loaded (%s)", url ? url : "null");
    beacon_string_free(url);

    CHECK(wait_for_frame(browser, tab, 10000), "a frame was composited");
    CHECK(write_ppm("/tmp/beacon-ffi.ppm", browser, tab), "the frame writes out as /tmp/beacon-ffi.ppm");

    /* ── 2. tabs open, close, and the last one holds ───────────────────── */
    printf("\nopening and closing tabs\n");
    BeaconTabId second = beacon_open_tab(browser, url_b);
    BeaconTabId third = beacon_open_tab(browser, url_b);
    pump(browser);
    CHECK(second != 0 && third != 0 && second != third, "each tab gets a distinct handle");
    CHECK(beacon_tab_count(browser) == 3, "three tabs are open (got %zu)", beacon_tab_count(browser));

    /* The strip order should hold the tabs we opened, in order. */
    CHECK(beacon_tab_at(browser, 0) == tab, "the first tab is at index 0");
    CHECK(beacon_tab_at(browser, 2) == third, "the third tab is at index 2");
    CHECK(beacon_tab_at(browser, 99) == 0, "an out-of-range index returns 0 rather than a stale handle");

    beacon_close_tab(browser, third);
    pump(browser);
    CHECK(beacon_tab_count(browser) == 2, "closing a tab leaves two (got %zu)", beacon_tab_count(browser));
    CHECK(beacon_tab_title(browser, third) == NULL, "the closed tab's handle no longer resolves");

    beacon_close_tab(browser, second);
    pump(browser);
    CHECK(beacon_tab_count(browser) == 1, "closing another leaves one");
    beacon_close_tab(browser, tab);
    pump(browser);
    CHECK(beacon_tab_count(browser) == 1, "the last tab refuses to close");

    /* ── 3. back and forward walk real history ─────────────────────────── */
    printf("\nsession history\n");
    beacon_activate_tab(browser, tab);
    CHECK(beacon_active_tab(browser) == tab, "the activated tab reports as active");
    CHECK(!beacon_tab_can_go_back(browser, tab), "a tab with one entry cannot go back");

    beacon_navigate(browser, tab, url_b);
    CHECK(settle(browser, tab, 15000), "the second page loads");
    CHECK(beacon_tab_can_go_back(browser, tab), "after a second page, back becomes available");

    beacon_back(browser);
    CHECK(settle(browser, tab, 15000), "going back settles");
    url = beacon_tab_url(browser, tab);
    CHECK(url && strstr(url, "beacon-ffi-a.html") != NULL, "back lands on the first page (%s)", url ? url : "null");
    beacon_string_free(url);
    CHECK(beacon_tab_can_go_forward(browser, tab), "forward is now available");

    beacon_forward(browser);
    CHECK(settle(browser, tab, 15000), "going forward settles");
    url = beacon_tab_url(browser, tab);
    CHECK(url && strstr(url, "beacon-ffi-b.html") != NULL, "forward returns to the second page (%s)", url ? url : "null");
    beacon_string_free(url);

    /* ── 4. scrolling changes what is drawn ────────────────────────────── */
    printf("\nscrolling\n");
    beacon_navigate(browser, tab, url_tall);
    CHECK(settle(browser, tab, 15000), "the tall page loads");
    CHECK(wait_for_frame(browser, tab, 10000), "the top of the page composites");
    unsigned long long top = frame_digest(browser, tab);

    /* Several notches, then let the engine re-render. */
    for (int i = 0; i < 12; i++) {
        beacon_scroll(browser, tab, 0.0f, 120.0f);
        sleep_ms(40);
        pump(browser);
    }
    sleep_ms(700);
    pump(browser);

    unsigned long long scrolled = frame_digest(browser, tab);
    CHECK(scrolled != 0, "a frame is still available after scrolling");
    CHECK(scrolled != top, "scrolling changes what is drawn");
    write_ppm("/tmp/beacon-ffi-scrolled.ppm", browser, tab);

    /* ── 5. zoom, pinning, reordering, reopening ───────────────────────── */
    printf("\nchrome state\n");
    CHECK(beacon_zoom(browser, tab) == 1.0f, "a fresh tab is at 100%%");
    beacon_set_zoom(browser, tab, 1.5f);
    CHECK(beacon_zoom(browser, tab) == 1.5f, "zoom is remembered");
    beacon_set_zoom(browser, tab, 99.0f);
    CHECK(beacon_zoom(browser, tab) == 5.0f, "zoom clamps at 500%% rather than accepting nonsense");
    beacon_set_zoom(browser, tab, 1.0f);
    sleep_ms(400);
    pump(browser);
    CHECK(frame_digest(browser, tab) != 0, "the page still renders after zooming back out");

    CHECK(!beacon_tab_is_pinned(browser, tab), "tabs start unpinned");
    beacon_set_tab_pinned(browser, tab, true);
    CHECK(beacon_tab_is_pinned(browser, tab), "a tab can be pinned");
    beacon_set_tab_pinned(browser, tab, false);
    CHECK(!beacon_tab_is_pinned(browser, tab), "and unpinned again");

    /* Reordering needs somewhere to move to. */
    BeaconTabId extra = beacon_open_tab(browser, url_a);
    pump(browser);
    CHECK(beacon_tab_at(browser, 1) == extra, "the new tab lands at index 1");
    beacon_move_tab(browser, extra, 0);
    CHECK(beacon_tab_at(browser, 0) == extra, "moving a tab to index 0 puts it first");

    beacon_close_tab(browser, extra);
    pump(browser);
    CHECK(beacon_tab_count(browser) == 1, "closing it leaves one tab");
    BeaconTabId reopened = beacon_reopen_closed_tab(browser);
    pump(browser);
    CHECK(reopened != 0 && beacon_tab_count(browser) == 2, "the closed tab comes back");
    beacon_close_tab(browser, reopened);
    pump(browser);

    /* Deliberately not asserting that the stack is now empty: section 2 closed tabs too,
     * and reopen is supposed to keep walking back through those. */

    /* Keyboard: the engine may or may not do anything visible with these on a static page,
     * but the ABI must survive them and the tab must keep rendering. */
    beacon_key_down(browser, tab, "a", "KeyA", 0);
    beacon_key_up(browser, tab, "a", "KeyA", 0);
    beacon_key_down(browser, tab, "Enter", NULL, BEACON_MOD_SHIFT);
    beacon_key_up(browser, tab, "Enter", NULL, BEACON_MOD_SHIFT);
    beacon_text_input(browser, tab, "hello");
    sleep_ms(300);
    pump(browser);
    CHECK(frame_digest(browser, tab) != 0, "the tab survives keyboard input");

    /* ── 6. bookmarks and downloads ────────────────────────────────────── */
    printf("\nbookmarks and downloads\n");
    CHECK(!beacon_tab_is_bookmarked(browser, tab), "a page starts unbookmarked");
    CHECK(beacon_toggle_bookmark(browser, tab), "bookmarking reports it is now on");
    CHECK(beacon_tab_is_bookmarked(browser, tab), "and the page reads back as bookmarked");
    CHECK(beacon_bookmark_count(browser) >= 1, "it appears in the bookmark list");

    /* Scan rather than assume index 0: a fresh profile is seeded with bookmarks of its
     * own, and the new one is appended after them. */
    int found = 0;
    for (size_t i = 0; i < beacon_bookmark_count(browser); i++) {
        char *url_i = beacon_bookmark_url(browser, i);
        if (url_i && strstr(url_i, "beacon-ffi") != NULL) {
            found = 1;
        }
        beacon_string_free(url_i);
    }
    CHECK(found, "the bookmarked page is in the list");
    CHECK(beacon_bookmark_url(browser, 999) == NULL, "an out-of-range bookmark is NULL, not a stale pointer");

    CHECK(!beacon_toggle_bookmark(browser, tab), "toggling again reports it is off");
    CHECK(!beacon_tab_is_bookmarked(browser, tab), "and it is gone");

    /* Internal pages are not bookmarkable, whatever the shell asks. */
    BeaconTabId internal = beacon_open_tab(browser, "gosub://home");
    settle(browser, internal, 10000);
    CHECK(!beacon_toggle_bookmark(browser, internal), "gosub:// pages refuse to be bookmarked");
    beacon_close_tab(browser, internal);
    pump(browser);

    /* History.
     *
     * The pages this test loads are file:// URLs, and the engine records a visit only for
     * http and https — internal pages and local files are deliberately not "places"
     * (worker.rs, on navigation finished). So this run cannot produce a positive hit
     * without a network, and the checks below cover the ABI's discipline instead: what an
     * empty query means, that a miss clears the previous result rather than leaving it
     * readable, and that rows are NULL out of range. The happy path is exercised by the
     * shell against a real profile. */
    CHECK(beacon_history_search(browser, "", 8) == 0, "an empty query matches nothing, not everything");
    CHECK(beacon_history_url(browser, 0) == NULL, "an empty query leaves no rows");

    size_t hits = beacon_history_search(browser, "example", 8);
    /* Whatever a profile happens to hold, count and rows must agree with each other. */
    if (hits > 0) {
        char *hit = beacon_history_url(browser, 0);
        CHECK(hit != NULL, "a reported hit has a readable URL");
        beacon_string_free(hit);
        CHECK(beacon_history_url(browser, hits) == NULL, "one past the last hit is NULL");
    } else {
        CHECK(beacon_history_url(browser, 0) == NULL, "no hits means no readable rows");
        CHECK(1, "no history in this profile, as expected for a file:// run");
    }
    CHECK(beacon_history_url(browser, 999) == NULL, "an out-of-range history row is NULL");

    CHECK(beacon_history_search(browser, "nothing-matches-this-xyzzy", 8) == 0, "a query with no matches returns 0");
    CHECK(beacon_history_url(browser, 0) == NULL, "and a miss clears the previous result");

    /* Developer panel: logs and timings.
     *
     * The engine has been parsing and rendering throughout this run, so its timing table
     * should not be empty. The log buffer may well be — the default level is warnings, and
     * a clean run produces none — so that is checked for consistency, not for content. */
    printf("\ndeveloper panel\n");
    size_t namespaces = beacon_timing_snapshot(browser);
    CHECK(namespaces > 0, "the engine recorded timings during this run (%zu namespaces)", namespaces);

    BeaconTiming timing;
    CHECK(beacon_timing_at(browser, 0, &timing), "the first row reads back");
    CHECK(timing.count > 0 && timing.total_us > 0, "it has a real count and total");
    CHECK(timing.min_us <= timing.avg_us && timing.avg_us <= timing.max_us, "min <= avg <= max holds");
    char *ns = beacon_timing_namespace(browser, 0);
    CHECK(ns != NULL && ns[0] != '\0', "and a namespace (%s)", ns ? ns : "null");
    beacon_string_free(ns);

    /* Slowest first, so the totals must not increase down the list. */
    int ordered = 1;
    uint64_t previous = timing.total_us;
    for (size_t i = 1; i < namespaces; i++) {
        BeaconTiming row;
        if (!beacon_timing_at(browser, i, &row)) {
            ordered = 0;
            break;
        }
        if (row.total_us > previous) {
            ordered = 0;
            break;
        }
        previous = row.total_us;
    }
    CHECK(ordered, "rows are ordered slowest first");

    BeaconTiming untouched = {.count = 4242};
    CHECK(!beacon_timing_at(browser, namespaces, &untouched), "one past the last row returns false");
    CHECK(untouched.count == 4242, "and leaves the caller's struct alone");

    beacon_timing_reset(browser);
    CHECK(beacon_timing_snapshot(browser) == 0, "resetting empties the table");

    size_t lines = beacon_log_snapshot(browser, 100);
    CHECK(beacon_log_message(browser, lines) == NULL, "one past the last log line is NULL");
    if (lines > 0) {
        char *message = beacon_log_message(browser, 0);
        CHECK(message != NULL, "a reported log line is readable");
        beacon_string_free(message);
        uint32_t level = beacon_log_level(browser, 0);
        CHECK(level >= BEACON_LOG_ERROR && level <= BEACON_LOG_TRACE, "its level is in range (%u)", level);
        CHECK(beacon_log_timestamp(browser, 0) > 0, "and a timestamp");
    } else {
        CHECK(beacon_log_message(browser, 0) == NULL, "no lines means nothing readable");
        CHECK(1, "no warnings logged this run, which is the default level");
    }
    beacon_log_clear(browser);
    CHECK(beacon_log_snapshot(browser, 100) == 0, "clearing empties the log");

    /* No download is in flight, so the queries must answer emptily rather than crash. */
    CHECK(beacon_download_count(browser) == 0, "no downloads yet");
    CHECK(beacon_download_at(browser, 0) == 0, "an out-of-range download id is 0");
    CHECK(beacon_download_filename(browser, 12345) == NULL, "an unknown download has no filename");
    CHECK(beacon_download_offer_url(browser, 12345) == NULL, "an unknown offer has no URL");
    CHECK(beacon_download_accept(browser, 12345, "/tmp/nope") == 0, "accepting an unknown offer returns 0");
    beacon_download_reject(browser, 12345); /* must not crash */

    /* Progress and favicon: a settled local page has neither, and must say so cleanly. */
    CHECK(beacon_tab_progress(browser, tab) == -1.0, "a settled tab reports no progress");
    size_t icon_len = 12345;
    const uint8_t *icon = beacon_tab_favicon(browser, tab, &icon_len);
    CHECK(icon == NULL && icon_len == 0, "a page with no favicon returns NULL and zeroes the length");

    beacon_free(browser);

    printf("\n%s (%d failure%s)\n", failures ? "FAILED" : "all checks passed", failures, failures == 1 ? "" : "s");
    return failures ? 1 : 0;
}
