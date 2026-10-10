# Process isolation in Beacon

Beacon GTK can run the Gosub engine with every component in its own sandboxed process.
This page says what that means, how to build and run it, how it is verified, and what is
known not to work yet. The engine's own account is `docs/process-isolation.md` in the
engine repository; this is the embedder's side.

## What runs where

| Process | What it does | Sandbox |
| --- | --- | --- |
| `gosub-beacon-gtk` (the broker) | The window, the engine's zones and tabs, navigation, compositing. Fetches nothing and parses no page content itself. | Writes confined to the profile, `~/Downloads`, `/dev/dri`, the dconf cache and the temp dir (Landlock); escalation syscalls denied (seccomp). |
| `gosub-net` | The network stack: every request, redirect, TLS handshake, cookie header. | Default-deny seccomp, filesystem scoped to resolver and CA paths. |
| `gosub-vault` | The cookie jars. The network process asks it directly; the broker never sees cookie values. | Default-deny seccomp. |
| `gosub-storage` | `localStorage` areas as files under the profile. | Default-deny seccomp. |
| `gosub-decoder` | One throwaway process per image decode. | Default-deny seccomp; exits with the image. |
| `gosub-forksrv` | The fork server: builds and warms the font system once, confines itself, and forks a renderer per site. | Default-deny seccomp, no file access. |
| `renderer-<id>` | One resident renderer per (zone, site): parses, styles, lays out and rasterises that site's pages, keeps them for scroll, hover and input. | Inherited from the fork server: no file access, no network, no exec, its own PID namespace, a memory ceiling. |

## What Beacon does to get it

The engine's embedder contract has five steps; Beacon takes all five.

1. `child_process::dispatch_with::<GtkConfig>()` is the first statement of `run()`. The engine spawns children by re-executing the Beacon binary with a role argument.
2. `lock_down_broker` right after, before any thread, the logger or the engine exist (`beacon-core::isolation`). This runs in every mode, `--single-process` included: there page content is parsed in Beacon's own process, so the confinement matters more, not less.
3. The `security.process_isolation` switch is set for the run before `start()`: on in an `isolation` build unless `--single-process`, on with `--isolated` in any build, off otherwise. The stored setting is left alone.
4. A forked rasteriser and a fully confinable font system: the `isolation` build feature switches `GtkConfig` to cosmic-text fonts and compiles in the Cairo tile rasteriser. Skia's own font system is fontconfig-backed, which the engine can only confine as a fresh process per render; cosmic-text gets the fork server and resident renderers.
5. `localStorage` through `ServiceLocalStore` in the isolated mode, so the storage process serves it; the plain build keeps the SQLite store. The two modes keep separate `localStorage` for the same profile.

`RendererCrashed` is logged; the engine replaces a dead renderer on the tab's next render.

## Build and run

```bash
cargo build --release --bin gosub-beacon-gtk --no-default-features --features gtk,isolation
./target/release/gosub-beacon-gtk https://example.org          # isolated by default
./target/release/gosub-beacon-gtk --single-process https://…    # the one-process engine
```

A default build (`--features gtk`) is the one-process engine; `--isolated` there still gets the network, vault, storage and decoder processes, but pages render in Beacon's process because the renderer tier needs the feature.

`pstree -p $(pgrep -x gosub-beacon-gt)` shows the tree. The log (`BEACON_LOG=info`) says what came up: "network stack running in a separate, sandboxed process", "cookie jars live in a separate, sandboxed vault process", "localStorage is served by a separate, sandboxed storage process", "renderer fork server ready (confinement tier: Full)".

## How it is verified

Three layers, each proving something the others cannot.

- **The engine's isolation suite**: `cargo test -p gosub_engine --test process_isolation` in the engine repository, 44 scenarios run through the `isolation-harness` binary under the broker lockdown. They cover the network process, the vault, the storage service, the decoder, the fork server, the renderer protocol (scroll, hover, input, resize, crash and replacement), and `no_process_finds_a_way_out_of_its_sandbox`. Sandbox correctness is proven here; Beacon cannot prove it and does not try.
- **The sandbox probes**: `gosub_sandbox`'s `sandbox-probe` binary applies each lockdown and attempts one escape per probe; the engine's tests run them. These are the negative controls.
- **Beacon's smoke test**: `scripts/isolation-smoke.sh`, run in CI as the `isolation-smoke` job. A `--single-process` run and an `--isolated` run of the same binary under Xvfb against the fixture page: the process tree (network, vault, storage, fork server, a renderer), the log lines above and the absence of every fallback warning, the page title recorded by the renderer process, typing, scrolling, a window narrowed and widened again and a link followed in both modes, and the two renders compared after every step. This proves Beacon's wiring, not the sandboxes.

## Known limits

- Same-site tabs share a renderer and render serially; a keystroke in one waits behind another's render.
- During a window drag the page shows at its old geometry until the renderer's resize pass lands; sizes that arrive while one is in flight collapse to the latest. Tiles are not scaled to the new size meanwhile.
- A re-layout after input is a full layout, in-process and out alike.
- Downloads can only be saved under `~/Downloads` (or the profile) while the broker is locked down.
- cgroup v2 memory delegation is unavailable on a default desktop; children fall back to rlimits, which the log says.
- Linux only. The egui frontend gets the service processes but never the renderer tier: Vello presents a GPU texture, and isolated renderers produce CPU tiles.
