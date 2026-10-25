# Known platform issues

Devices and environments where Beacon is known to break, why, and what has to happen before
it is fixed. One open issue matters most: Beacon cannot draw pages on any Android device with
an Arm Mali GPU until Vello ships a fix (KI-001).

Add new entries at the end of the table with the next free ID, and give each one the
platform, the symptom as a user sees it, the cause, the status, and when to look again.

| ID | Platform | Symptom | Status |
| --- | --- | --- | --- |
| [KI-001](#ki-001-mali-gpus-lose-the-vulkan-device) | Android, any Mali GPU | GPU device lost on the first page; blank page | Open, waiting for Vello |
| [KI-002](#ki-002-older-gpus-reject-the-default-wgpu-limits) | Android, older GPUs | Beacon exits right after launch | Fixed |
| [KI-003](#ki-003-opengl-es-cannot-run-vello) | OpenGL ES only | Vello cannot run | Won't fix |
| [KI-004](#ki-004-32-bit-only-android-devices) | Android, 32-bit only | APK does not install; settings lost | Fixed |

## KI-001: Mali GPUs lose the Vulkan device

On an Android device with an Arm Mali GPU, the first page Beacon draws kills the GPU and the
page stays blank. The bug is in Vello, and a fix is open upstream. Beacon waits for it rather
than carrying a fork of Vello.

- **Seen on:** Samsung Galaxy Tab A 10.1 (SM-T510, Mali-G71, Android 11). Upstream reports the
  same on a Pixel 9 Pro and a Pixel 10a, so current Mali phones are affected too. Adreno GPUs
  work.
- **Symptom:** a blank page, and `VK_ERROR_DEVICE_LOST` in `adb logcat`. On a simple page it
  happens only some of the time; on a real page, every time.
- **Cause:** six lookups in Vello's `tile_alloc`, `coarse`, `backdrop_dyn` and `fine` shaders
  have the form `select(0u, arr[i - 1u], i > 0u)`. Both arms are evaluated, so thread 0 reads
  `arr[0u - 1u]`, an index of about 4 billion. Vello builds its shaders without bounds checks,
  and Mali keeps workgroup memory in ordinary memory, so that read is a wild access and the
  driver drops the device.
- **Fix:** [vello#1817](https://github.com/linebender/vello/pull/1817) clamps the index in
  the discarded arm. With those six lines applied to Beacon's pinned Vello (`1b24e77`), the
  SM-T510 rendered `gosub://home` and Hacker News in both orientations without a device loss.
- **Why it is not fixed here:** #1817 is built on Vello main, which is past the 0.8 release
  Beacon pins and moves Vello to 0.9. Taking it means either a fork of Vello or porting the
  engine to 0.9 early.
- **Workaround:** none. A Mali device cannot run Beacon until this lands.
- **Revisit when:** #1817 is merged and released. Then move the Vello pin in
  `[patch.crates-io]` (here and in the engine) to a release that contains it, and check on a
  Mali device.
- **Upstream:** [vello#1816](https://github.com/linebender/vello/issues/1816) (the bug),
  [vello#1815](https://github.com/linebender/vello/issues/1815) (shaders built unchecked),
  [vello#1817](https://github.com/linebender/vello/pull/1817) (the fix).

## KI-002: older GPUs reject the default wgpu limits

Beacon exited right after launch on GPUs below wgpu's default limits.

- **Seen on:** SM-T510 (Mali-G71).
- **Symptom:** the app closes at once; the log says
  `Limit 'max_color_attachments' value 8 is better than allowed 4`.
- **Cause:** egui-wgpu asks for `wgpu::Limits::default()` when it opens the device, and many
  mobile GPUs allow fewer colour attachments than that.
- **Fix:** the Android entry asks for the adapter's own limits instead
  (`crates/beacon-egui/src/android.rs`). On a Mali device this only gets Beacon past startup;
  KI-001 still blanks the page.

## KI-003: OpenGL ES cannot run Vello

Vello does not work on wgpu's OpenGL ES backend, so a device with only OpenGL ES cannot run
Beacon, and OpenGL ES is no fallback for KI-001.

- **Cause:** Vello's compute shaders use workgroups of 256 threads, and OpenGL ES guarantees
  only 128.
- **Status:** won't fix; it is a limit of Vello's design. Android devices Beacon targets have
  Vulkan.

## KI-004: 32-bit-only Android devices

Some devices run a 32-bit Android on 64-bit hardware; the SM-T510's Android 11 reports only
`armeabi-v7a`.

- **APK does not install** (`INSTALL_FAILED_NO_MATCHING_ABIS`), since the default build is
  arm64 only. Fix: build a 32-bit APK with
  `ANDROID_ABI=armeabi-v7a scripts/android-apk.sh` (needs
  `rustup target add armv7-linux-androideabi`).
- **All engine settings lost:** an engine default of `u:4294967296` overflowed a 32-bit
  `usize`, and the engine dropped its whole settings schema. Fixed in
  [gosub-engine#1465](https://github.com/gosub-io/gosub-engine/pull/1465).

## Tested devices

| Device | SoC / GPU | Android | Result |
| --- | --- | --- | --- |
| Samsung Galaxy Tab A 10.1 (SM-T510) | Exynos 7904 / Mali-G71 | 11, 32-bit only | Blank page (KI-001) |
| Samsung Galaxy Tab A9+ | Snapdragon / Adreno | arm64 | Works |
| Fairphone 6 | Snapdragon / Adreno | 16, arm64 | Works |
