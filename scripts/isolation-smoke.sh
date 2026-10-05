#!/usr/bin/env bash
# Smoke test for the renderer tier: does `gosub-beacon-gtk --isolated` bring up the
# component processes, render a page out of process, and paint the same thing the
# in-process path paints?
#
# Two runs of the same binary against a fixture page served from 127.0.0.1, both under
# Xvfb: a plain run and an `--isolated` run. The plain run must stay single-process. The
# isolated run must show the network, vault, fork server and a renderer process, log the
# fork server as ready at the Full tier, record the page title (it comes back from the
# renderer, so it proves the remote render completed) and crash nothing. Then the two
# screenshots are compared: the remote renderer rasterizes with Cairo where the in-process
# path uses Skia, so antialiasing differs by a few pixels per glyph edge and the check is
# a share of differing pixels under a colour fuzz, not an exact match.
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

# Run the binary in one mode until the page has rendered and the screen has settled,
# capture it, and stop it. Sets $log, $shot, $profile for the caller.
run_mode() {
    local mode=$1
    shift
    profile="$OUT/profile-$mode"
    log="$OUT/$mode.log"
    shot="$OUT/$mode.png"
    rm -rf "$profile"
    mkdir -p "$profile"

    # Its own session bus: `io.gosub.beacon` is a single-instance application, and a
    # Beacon already running on the desktop's bus would otherwise take the URL and
    # leave this run with nothing.
    local launcher=()
    command -v dbus-run-session >/dev/null && launcher=(dbus-run-session --)
    BEACON_LOG=info GDK_BACKEND=x11 setsid "${launcher[@]}" "$BIN" "$@" --user-data-dir "$profile" "$URL" >"$log" 2>&1 &
    local pgid=$!
    cleanup_pids+=("$pgid")

    local i
    for i in $(seq 1 60); do
        [ -n "$(xdotool search --name 'Gosub Beacon' 2>/dev/null | head -1)" ] && break
        sleep 1
    done
    if [ "$i" -ge 60 ]; then
        fail "$mode: no window after 60 s"
    fi

    local title=""
    for i in $(seq 1 90); do
        title=$(recorded_title "$profile")
        [ "$title" = "$EXPECTED_TITLE" ] && break
        sleep 1
    done
    if [ "$title" = "$EXPECTED_TITLE" ]; then
        pass "$mode: page title recorded after ${i} s (the render completed)"
    else
        fail "$mode: title not recorded within 90 s (got '${title}')"
    fi

    # The chrome shows the title a moment after it is recorded; wait for it so the two
    # captures compare the same window state.
    local wid
    wid=$(xdotool search --name 'Gosub Beacon' 2>/dev/null | tail -1)
    for i in $(seq 1 30); do
        case $(xdotool getwindowname "$wid" 2>/dev/null) in
            *"$EXPECTED_TITLE"*) break ;;
        esac
        sleep 1
    done

    # The caller's checks on the live process tree.
    if [ "$mode" = isolated ]; then
        check_isolated_tree
    else
        check_plain_tree
    fi

    # Settled: two consecutive captures a second apart agree exactly.
    local prev="$OUT/$mode-prev.png"
    import -window root "$prev"
    for i in $(seq 1 30); do
        sleep 1
        import -window root "$shot"
        if [ "$(compare -metric AE "$prev" "$shot" /dev/null 2>&1 | tr -d '\n')" = "0" ]; then
            break
        fi
        mv "$shot" "$prev"
    done
    [ -f "$shot" ] || mv "$prev" "$shot"
    rm -f "$prev"
    note "$mode: screen settled after ${i} s"

    kill -TERM -- "-$pgid" 2>/dev/null
    for i in $(seq 1 20); do
        kill -0 -- "-$pgid" 2>/dev/null || break
        sleep 0.5
    done
    kill -KILL -- "-$pgid" 2>/dev/null
}

procs() { ps -eo comm= ; }

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
    for name in gosub-net gosub-vault gosub-forksrv; do
        procs | grep -qx "$name" || missing="$missing $name"
    done
    procs | grep -q '^renderer-' || missing="$missing renderer-*"
    if [ -z "$missing" ]; then
        pass "isolated: network, vault, fork server and a renderer are running"
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
run_mode plain
plain_shot=$shot
check_log plain "$log" \
    "security.process_isolation is off" \
    '!network stack running in a separate' \
    '!panicked' \
    '!SIGSYS: blocked syscall'

echo "== isolated run"
run_mode isolated --isolated
iso_shot=$shot
check_log isolated "$log" \
    "network stack running in a separate, sandboxed process" \
    "renderer fork server ready (confinement tier: Full)" \
    "landlock active" \
    '!no forked_tile_rasterizer' \
    '!Renderer for' \
    '!panicked' \
    '!SIGSYS: blocked syscall'
if grep -q 'no forked_tile_rasterizer' "$log"; then
    note "the binary was built without --features isolation"
fi

echo "== compare"
if [ -f "$plain_shot" ] && [ -f "$iso_shot" ]; then
    share=$(diff_share "$plain_shot" "$iso_shot" "$OUT/diff.png")
    if python3 -c "import sys; sys.exit(0 if float('$share') <= float('$MAX_DIFF') else 1)"; then
        pass "isolated render matches the in-process one (differing pixels: $share, limit $MAX_DIFF)"
    else
        fail "isolated render differs from the in-process one (differing pixels: $share, limit $MAX_DIFF); see $OUT/diff.png"
    fi
fi

if [ "$failures" -eq 0 ]; then
    echo "== OK (artefacts in $OUT)"
    exit 0
fi
echo "== $failures failure(s); artefacts in $OUT"
exit 1
