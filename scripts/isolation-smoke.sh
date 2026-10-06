#!/usr/bin/env bash
# Smoke test for the renderer tier: does `gosub-beacon-gtk --isolated` bring up the
# component processes, render a page out of process, and paint the same thing the
# in-process path paints?
#
# Two runs of the same binary against a fixture page served from 127.0.0.1, both under
# Xvfb: a `--single-process` run and an `--isolated` run (the isolation build's default). Each run also types into a field, scrolls
# down and back, and follows a link, with a capture after every step. The plain run must stay single-process. The
# isolated run must show the network, vault, fork server and a renderer process, log the
# fork server as ready at the Full tier, record the page title (it comes back from the
# renderer, so it proves the remote render completed) and crash nothing. Then the two
# screenshots of every step are compared: the remote renderer rasterizes with Cairo where
# the in-process path uses Skia, so antialiasing differs by a few pixels per glyph edge
# and the check is a share of differing pixels under a colour fuzz, not an exact match.
#
# Needs: a binary built with `--features isolation` (Linux), Xvfb (or a DISPLAY to use),
# xdotool, ImageMagick (`import`, `compare`), python3. Run it under `xvfb-run -a` in CI.
#
#   scripts/isolation-smoke.sh [path/to/gosub-beacon-gtk]
#
# SMOKE_OUT names the artefact directory (default: a fresh temp dir, kept on failure).
# SMOKE_MAX_DIFF is the allowed share of differing pixels (default 0.02).

set -u

BIN=${1:-target/debug/gosub-beacon-gtk}
FIXTURE_DIR=$(cd "$(dirname "$0")/.." && pwd)/tests/fixtures/isolation
OUT=${SMOKE_OUT:-$(mktemp -d /tmp/beacon-smoke.XXXXXX)}
MAX_DIFF=${SMOKE_MAX_DIFF:-0.02}
EXPECTED_TITLE="Isolation smoke"
SECOND_TITLE="Isolation smoke, page two"
# The fixture's targets, in CSS px below the band's top edge and right of its left edge
# (see tests/fixtures/isolation/index.html). Xvfb renders at DPR 1, so CSS px are pixels.
FIELD_Y=160
LINK_Y=324
TARGET_X=200
STEPS="initial typed scrolled restored page2"
mkdir -p "$OUT"

failures=0
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; failures=$((failures + 1)); }
note() { echo "      $*"; }

for tool in xdotool import compare python3; do
    if ! command -v "$tool" >/dev/null; then
        echo "missing tool: $tool" >&2
        exit 2
    fi
done
if [ ! -x "$BIN" ]; then
    echo "no binary at $BIN; build with: cargo build --bin gosub-beacon-gtk --no-default-features --features gtk,isolation" >&2
    exit 2
fi
BIN=$(readlink -f "$BIN")

cleanup_pids=()
cleanup() {
    for p in "${cleanup_pids[@]:-}"; do
        [ -n "$p" ] && kill -TERM -- "-$p" 2>/dev/null
    done
}
trap cleanup EXIT

# A display of our own when the caller gave none (xvfb-run sets DISPLAY).
if [ -z "${DISPLAY:-}" ]; then
    if ! command -v Xvfb >/dev/null; then
        echo "no DISPLAY and no Xvfb" >&2
        exit 2
    fi
    fdfile=$(mktemp)
    setsid Xvfb -displayfd 3 -screen 0 1400x900x24 3>"$fdfile" >/dev/null 2>&1 &
    cleanup_pids+=("$!")
    for _ in $(seq 1 50); do
        [ -s "$fdfile" ] && break
        sleep 0.2
    done
    DISPLAY=":$(cat "$fdfile")"
    export DISPLAY
    note "Xvfb on $DISPLAY"
fi

# The fixture, served from loopback on a port the OS picks.
setsid python3 -u -m http.server --bind 127.0.0.1 --directory "$FIXTURE_DIR" 0 >"$OUT/server.log" 2>&1 &
cleanup_pids+=("$!")
PORT=
for _ in $(seq 1 50); do
    PORT=$(sed -n 's/.*port \([0-9]*\).*/\1/p' "$OUT/server.log" | head -1)
    [ -n "$PORT" ] && break
    sleep 0.2
done
if [ -z "$PORT" ]; then
    echo "fixture server did not start:" >&2
    cat "$OUT/server.log" >&2
    exit 2
fi
URL="http://127.0.0.1:$PORT/"
note "fixture at $URL"

# The title the page records once it has rendered, read from the profile's places
# database. Empty until then.
recorded_title() {
    python3 - "$1" <<'EOF'
import sqlite3, sys, os
db = os.path.join(sys.argv[1], "places.db")
if not os.path.exists(db):
    sys.exit(0)
try:
    con = sqlite3.connect(db)
    row = con.execute("select title from visits order by last_visit desc limit 1").fetchone()
    print(row[0] if row else "")
except sqlite3.Error:
    pass
EOF
}

# Pixels that differ between two captures, as a share of all pixels, under the fuzz.
diff_share() {
    local ae
    ae=$(compare -metric AE -fuzz 10% "$1" "$2" "${3:-/dev/null}" 2>&1 | tr -d '\n')
    python3 - "$ae" "$1" <<'EOF'
import sys, struct
ae = float(sys.argv[1].split()[0])
with open(sys.argv[2], "rb") as f:
    f.seek(16)
    w, h = struct.unpack(">II", f.read(8))
print(f"{ae / (w * h):.5f}")
EOF
}

# Capture the root window once the screen has settled: two captures a second apart
# that differ in almost nothing (a caret may blink between them).
settle_capture() {
    local out=$1 prev="$1.prev.png" i
    import -window root "$prev"
    for i in $(seq 1 30); do
        sleep 1
        import -window root "$out"
        if python3 -c "import sys; sys.exit(0 if float('$(diff_share "$prev" "$out")') <= 0.0005 else 1)"; then
            break
        fi
        mv "$out" "$prev"
    done
    [ -f "$out" ] || mv "$prev" "$out"
    rm -f "$prev"
    note "settled after ${i} s: $(basename "$out")"
}

# Where the page's band starts in the capture: its first row and column with the band
# colour, which is where the fixture's CSS offsets are measured from.
page_origin() {
    local capture=$1
    PAGE_TOP=$(convert "$capture" -crop 1x900+700+0 +repage txt:- | grep -m1 -n '#2B3A67' | cut -d: -f1)
    PAGE_TOP=$((PAGE_TOP - 2))
    PAGE_LEFT=$(convert "$capture" -crop 1400x1+0+$((PAGE_TOP + 10)) +repage txt:- | grep -m1 -n '#2B3A67' | cut -d: -f1)
    PAGE_LEFT=$((PAGE_LEFT - 2))
}

# Wait until the profile records `title` and the window shows it.
wait_for_title() {
    local title=$1 i found=""
    for i in $(seq 1 90); do
        [ "$(recorded_title "$profile")" = "$title" ] && found=1 && break
        sleep 1
    done
    [ -n "$found" ] || return 1
    local wid
    wid=$(xdotool search --name 'Gosub Beacon' 2>/dev/null | tail -1)
    for i in $(seq 1 30); do
        case $(xdotool getwindowname "$wid" 2>/dev/null) in
            *"$title"*) break ;;
        esac
        sleep 1
    done
    return 0
}

# Run the binary in one mode through every step, capturing each, and stop it. Sets $log
# and $profile for the caller; captures land at $OUT/$mode-$step.png.
run_mode() {
    local mode=$1
    shift
    profile="$OUT/profile-$mode"
    log="$OUT/$mode.log"
    rm -rf "$profile"
    mkdir -p "$profile"

    # Its own session bus: `io.gosub.beacon` is a single-instance application, and a
    # Beacon already running on the desktop's bus would otherwise take the URL and
    # leave this run with nothing.
    local launcher=()
    command -v dbus-run-session >/dev/null && launcher=(dbus-run-session --)
    BEACON_LOG=info GDK_BACKEND=x11 setsid "${launcher[@]}" "$BIN" "$@" --user-data-dir "$profile" "$URL" >"$log" 2>&1 &
    pgid=$!
    cleanup_pids+=("$pgid")

    local i
    for i in $(seq 1 60); do
        [ -n "$(xdotool search --name 'Gosub Beacon' 2>/dev/null | head -1)" ] && break
        sleep 1
    done
    if [ "$i" -ge 60 ]; then
        fail "$mode: no window after 60 s"
    fi

    if wait_for_title "$EXPECTED_TITLE"; then
        pass "$mode: page title recorded (the render completed)"
    else
        fail "$mode: title not recorded within 90 s (got '$(recorded_title "$profile")')"
    fi

    # The caller's checks on the live process tree.
    if [ "$mode" = isolated ]; then
        check_isolated_tree
    else
        check_plain_tree
    fi

    settle_capture "$OUT/$mode-initial.png"
    page_origin "$OUT/$mode-initial.png"
    if [ "$PAGE_TOP" -le 0 ] || [ "$PAGE_LEFT" -lt 0 ]; then
        fail "$mode: the page band was not found in the capture; skipping interaction"
        kill -TERM -- "-$pgid" 2>/dev/null
        return
    fi
    note "$mode: page at $PAGE_LEFT,$PAGE_TOP"

    # Type into the field.
    xdotool mousemove $((PAGE_LEFT + TARGET_X)) $((PAGE_TOP + FIELD_Y)) click 1
    sleep 0.5
    xdotool type --delay 80 "hello"
    settle_capture "$OUT/$mode-typed.png"
    if [ "$(diff_share "$OUT/$mode-initial.png" "$OUT/$mode-typed.png")" = "0.00000" ]; then
        fail "$mode: typing changed nothing on screen"
    else
        pass "$mode: typing into the field repainted"
    fi

    # Scroll down three notches, then back up.
    xdotool mousemove $((PAGE_LEFT + 700)) $((PAGE_TOP + 600)) click --repeat 3 --delay 150 5
    settle_capture "$OUT/$mode-scrolled.png"
    local moved
    moved=$(diff_share "$OUT/$mode-typed.png" "$OUT/$mode-scrolled.png")
    if python3 -c "import sys; sys.exit(0 if float('$moved') >= 0.02 else 1)"; then
        pass "$mode: the page scrolled (differing pixels: $moved)"
    else
        fail "$mode: the page did not scroll (differing pixels: $moved)"
    fi
    xdotool click --repeat 3 --delay 150 4
    settle_capture "$OUT/$mode-restored.png"
    local back
    back=$(diff_share "$OUT/$mode-typed.png" "$OUT/$mode-restored.png")
    if python3 -c "import sys; sys.exit(0 if float('$back') <= float('$MAX_DIFF') else 1)"; then
        pass "$mode: scrolling back restored the page (differing pixels: $back)"
    else
        fail "$mode: scrolling back did not restore the page (differing pixels: $back)"
    fi

    # Follow the link.
    xdotool mousemove $((PAGE_LEFT + TARGET_X)) $((PAGE_TOP + LINK_Y)) click 1
    if wait_for_title "$SECOND_TITLE"; then
        pass "$mode: the link navigated to the second page"
    else
        fail "$mode: the second page's title was not recorded within 90 s"
    fi
    settle_capture "$OUT/$mode-page2.png"

    kill -TERM -- "-$pgid" 2>/dev/null
    for i in $(seq 1 20); do
        kill -0 -- "-$pgid" 2>/dev/null || break
        sleep 0.5
    done
    kill -KILL -- "-$pgid" 2>/dev/null
}

# The processes of the run under test only: everything in its session (it was started
# with setsid, and the engine's children inherit the session), so a Beacon running on
# the desktop at the same time does not count.
procs() { ps -o comm= -s "$pgid" 2>/dev/null; }

check_plain_tree() {
    if procs | grep -qE '^(gosub-net|gosub-vault|gosub-forksrv|renderer-)'; then
        fail "plain: component processes are running without --isolated"
        procs | grep -E '^(gosub-net|gosub-vault|gosub-forksrv|renderer-)' | sed 's/^/      /'
    else
        pass "plain: no component processes"
    fi
}

check_isolated_tree() {
    local missing=""
    for name in gosub-net gosub-vault gosub-storage gosub-forksrv; do
        procs | grep -qx "$name" || missing="$missing $name"
    done
    procs | grep -q '^renderer-' || missing="$missing renderer-*"
    if [ -z "$missing" ]; then
        pass "isolated: network, vault, storage, fork server and a renderer are running"
    else
        fail "isolated: missing process(es):$missing"
    fi
}

check_log() {
    local mode=$1 file=$2
    shift 2
    local pattern
    for pattern in "$@"; do
        case $pattern in
            !*)
                if grep -q -- "${pattern#!}" "$file"; then
                    fail "$mode: log contains '${pattern#!}'"
                    grep -- "${pattern#!}" "$file" | head -3 | sed 's/^/      /'
                else
                    pass "$mode: log has no '${pattern#!}'"
                fi
                ;;
            *)
                if grep -q -- "$pattern" "$file"; then
                    pass "$mode: log says '$pattern'"
                else
                    fail "$mode: log lacks '$pattern'"
                fi
                ;;
        esac
    done
}

echo "== plain run"
run_mode plain --single-process
check_log plain "$log" \
    "security.process_isolation is off" \
    '!network stack running in a separate' \
    '!panicked' \
    '!SIGSYS: blocked syscall'

echo "== isolated run"
run_mode isolated --isolated
check_log isolated "$log" \
    "network stack running in a separate, sandboxed process" \
    "renderer fork server ready (confinement tier: Full)" \
    "localStorage is served by a separate, sandboxed storage process" \
    "landlock active" \
    '!storage service could not start' \
    '!could not apply parent-side confinement to the decoder' \
    '!falling back to in-process' \
    '!no forked_tile_rasterizer' \
    '!Renderer for' \
    '!panicked' \
    '!SIGSYS: blocked syscall'
if grep -q 'no forked_tile_rasterizer' "$log"; then
    note "the binary was built without --features isolation"
fi

echo "== compare"
for step in $STEPS; do
    plain_shot="$OUT/plain-$step.png"
    iso_shot="$OUT/isolated-$step.png"
    if [ ! -f "$plain_shot" ] || [ ! -f "$iso_shot" ]; then
        fail "$step: a capture is missing"
        continue
    fi
    share=$(diff_share "$plain_shot" "$iso_shot" "$OUT/diff-$step.png")
    if python3 -c "import sys; sys.exit(0 if float('$share') <= float('$MAX_DIFF') else 1)"; then
        pass "$step: isolated matches in-process (differing pixels: $share, limit $MAX_DIFF)"
    else
        fail "$step: isolated differs from in-process (differing pixels: $share, limit $MAX_DIFF); see $OUT/diff-$step.png"
    fi
done

if [ "$failures" -eq 0 ]; then
    echo "== OK (artefacts in $OUT)"
    exit 0
fi
echo "== $failures failure(s); artefacts in $OUT"
exit 1
