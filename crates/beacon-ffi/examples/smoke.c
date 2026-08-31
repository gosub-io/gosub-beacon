/*
 * The first consumer of the Beacon C ABI — deliberately in C, and deliberately headless.
 *
 * If the boundary is wrong, finding out here costs a compile. Finding out through Xcode
 * costs an afternoon. So before anyone writes Swift: open a tab, drive the event loop the
 * way a real shell would, and write the rendered page out as a PPM you can actually look
 * at.
 *
 *   make -C crates/beacon-ffi run
 *
 * Exits non-zero if the page never renders, so it works as a smoke test as well as a
 * demonstration.
 */

/* nanosleep is POSIX, and -std=c11 alone does not expose it. */
#define _POSIX_C_SOURCE 199309L

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "beacon.h"

/* A shell would do this on its run loop; here a plain sleep stands in for one. */
static void tick(void) {
    struct timespec t = {.tv_sec = 0, .tv_nsec = 50 * 1000 * 1000}; /* 50ms */
    nanosleep(&t, NULL);
}

static const char *kind_name(BeaconEventKind kind) {
    switch (kind) {
        case BEACON_REDRAW: return "redraw";
        case BEACON_TABS_CHANGED: return "tabs-changed";
        case BEACON_ACTIVE_TAB_CHANGED: return "active-tab-changed";
        case BEACON_TITLE_CHANGED: return "title";
        case BEACON_URL_CHANGED: return "url";
        case BEACON_LOADING_CHANGED: return "loading";
        case BEACON_PROGRESS: return "progress";
        case BEACON_FAVICON_CHANGED: return "favicon";
        case BEACON_NAV_STATE_CHANGED: return "nav-state";
        case BEACON_HOVER_URL: return "hover";
        case BEACON_CURSOR_CHANGED: return "cursor";
        case BEACON_DOWNLOAD_OFFERED: return "download";
        case BEACON_TAB_CRASHED: return "crashed";
        case BEACON_LOG: return "log";
        default: return "?";
    }
}

/* BGRA premultiplied -> binary PPM, so the result can be eyeballed with any image viewer. */
static int write_ppm(const char *path, const BeaconFrame *frame) {
    FILE *f = fopen(path, "wb");
    if (!f) {
        return 0;
    }
    fprintf(f, "P6\n%u %u\n255\n", frame->width, frame->height);
    for (uint32_t y = 0; y < frame->height; y++) {
        const uint8_t *row = frame->pixels + (size_t)y * frame->stride;
        for (uint32_t x = 0; x < frame->width; x++) {
            const uint8_t *px = row + (size_t)x * 4;
            /* BGRA in memory; PPM wants RGB. */
            fputc(px[2], f);
            fputc(px[1], f);
            fputc(px[0], f);
        }
    }
    fclose(f);
    return 1;
}

int main(int argc, char **argv) {
    const char *url = argc > 1 ? argv[1] : "https://example.com";
    const char *out = argc > 2 ? argv[2] : "/tmp/beacon-ffi.ppm";

    BeaconConfig config = {.user_data_dir = "/tmp/beacon-ffi-profile", .private_mode = false};
    BeaconBrowser *browser = beacon_new(&config);
    if (!browser) {
        fprintf(stderr, "beacon_new failed\n");
        return 1;
    }

    BeaconTabId tab = beacon_open_tab(browser, url);
    if (tab == 0) {
        fprintf(stderr, "could not open %s\n", url);
        beacon_free(browser);
        return 1;
    }
    printf("opened tab %llu on %s\n", (unsigned long long)tab, url);

    /* Nothing renders until the engine knows how big the page is. A real shell sends this
     * whenever its view resizes. */
    beacon_set_viewport(browser, tab, 1024, 768);

    /* The run loop: pump events, notice when a frame is ready, give up after a while. */
    BeaconEvent events[32];
    int rendered = 0;
    int redraws = 0;
    for (int i = 0; i < 300 && !rendered; i++) { /* ~15s */
        size_t n;
        while ((n = beacon_poll_events(browser, events, 32)) > 0) {
            for (size_t e = 0; e < n; e++) {
                BeaconEvent *ev = &events[e];
                if (ev->kind == BEACON_REDRAW) {
                    redraws++;
                    continue; /* too noisy to print */
                }
                printf("  [%s] tab=%llu", kind_name(ev->kind), (unsigned long long)ev->tab);
                if (ev->text) {
                    printf(" \"%s\"", ev->text);
                }
                if (ev->number != 0.0) {
                    printf(" (%.2f)", ev->number);
                }
                printf("\n");
            }
        }

        BeaconFrame frame;
        if (beacon_acquire_frame(browser, tab, &frame)) {
            printf("frame: %ux%u dpr=%u stride=%u after %d redraw(s)\n", frame.width, frame.height, frame.dpr, frame.stride, redraws);
            rendered = write_ppm(out, &frame);
            beacon_release_frame(browser, tab);
            if (rendered) {
                printf("wrote %s\n", out);
            } else {
                fprintf(stderr, "could not write %s\n", out);
            }
        }
        tick();
    }

    /* Prove the query side too: the shell asks rather than remembering. */
    char *title = beacon_tab_title(browser, tab);
    char *current = beacon_tab_url(browser, tab);
    printf("tabs=%zu active=%llu title=\"%s\" url=\"%s\" back=%d forward=%d\n",
           beacon_tab_count(browser),
           (unsigned long long)beacon_active_tab(browser),
           title ? title : "",
           current ? current : "",
           beacon_tab_can_go_back(browser, tab),
           beacon_tab_can_go_forward(browser, tab));
    beacon_string_free(title);
    beacon_string_free(current);

    beacon_free(browser);
    if (!rendered) {
        fprintf(stderr, "no frame was ever composited\n");
        return 1;
    }
    return 0;
}
