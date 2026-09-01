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
 *
 *   make -C crates/beacon-ffi run
 *
 * Test pages are written to /tmp and loaded over file://, so the run is deterministic and
 * needs no network. Exits non-zero if any check fails.
 */

/* nanosleep is POSIX, and -std=c11 alone does not expose it. */
#define _POSIX_C_SOURCE 199309L

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

    /* ── 1. a page loads and composites ────────────────────────────────── */
    printf("\nloading and rendering\n");
    BeaconTabId tab = beacon_open_tab(browser, url_a);
    CHECK(tab != 0, "opening a tab returns a handle");
    beacon_set_viewport(browser, tab, 1024, 768);
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

    beacon_free(browser);

    printf("\n%s (%d failure%s)\n", failures ? "FAILED" : "all checks passed", failures, failures == 1 ? "" : "s");
    return failures ? 1 : 0;
}
