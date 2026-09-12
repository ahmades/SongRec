# Now Playing regression checks

Run the non-display suite with `cargo test --release --all-targets`.

The ignored GTK tests require a GTK4 display. Run each filter in its own process
(GTK initialization is thread-affine), with `--ignored --test-threads=1`:

- `settings_controls_are_labelled_and_slider_gestures_do_not_retain_widgets`
- `deferred_artwork_resumes_and_transitions_with_metadata`
- `artwork_layout_callbacks_do_not_retain_containers`
- `mapped_transitions_retain_crossfade_scene_and_bound_slide_allocations`
- `settings_views_stay_in_sync_and_immediate_quit_preserves_sliders`

Use `GTK_A11Y=test` for the accessible-label check; `GTK_A11Y=none` disables
the relations that test is intended to inspect.

For example, on the desktop:

```sh
cargo test --release mapped_transitions_retain_crossfade_scene_and_bound_slide_allocations -- --ignored --test-threads=1
```

For an isolated display, start **gtk4-broadwayd**, not the GTK3 `broadwayd`, on an
unused port/display, then set `GDK_BACKEND=broadway`, `BROADWAY_DISPLAY=:42`, and
`GSK_RENDERER=cairo` on each test command. For example, a server can be started
with `gtk4-broadwayd -a 127.0.0.1 -p 28123 :42`. Allow it to finish starting before
launching tests and stop that server when testing is complete.

Use a native display for the animation-completion tests: with GTK 4.22.4, the
slide reveal leg stalled on Broadway without a connected browser, while all
effects completed on X11 and Wayland. A bare Broadway server is not a sufficient substitute
for the native animation check.

## Real-pointer context-menu check

Synthetic GTK signal emission does **not** exercise popup grabs or event routing.
The pure pointer-action test therefore does not replace this check. Repeat on the
supported native backends (Wayland and X11), windowed and fullscreen:

1. Open Now Playing and right-click the canvas.
2. Click the Display mode dropdown to open its nested popup.
3. Click the same dropdown again to close it without selecting a different mode.
4. Right-click the canvas outside the outer context menu. The menu must close,
   and the same click must not reopen it.
5. Repeat with a left-click outside, with a selected different mode, and with the
   transition-effect dropdown.
6. Open the menu with Menu or Shift+F10. Navigate controls with Tab, operate them
   with the keyboard, and close with Escape.
7. Verify each setting changed in either interface is reflected in the other;
   adjust a slider and immediately quit, then confirm its value after restart.
8. In each display mode, double-click the canvas with the primary mouse button
   to enter fullscreen, then again to exit. The menu's fullscreen button must
   stay in sync. Single/secondary clicks and double-clicks inside the settings
   menu must not toggle fullscreen; F11 must still work.

Run with `-v` to include the menu pointer/close diagnostics. Keep backend details
and the exact dropdown open/close sequence with any failure report.

### Automated private-desktop check

`real_pointer_dismisses_menu_after_using_nested_dropdowns` sends actual pointer
and keyboard events through a private Mutter compositor. It covers both mouse
buttons, both dropdowns, selecting an item and toggling the dropdown closed,
non-fullscreen/fullscreen, and Menu/Shift+F10, Tab, slider keys, and Escape.
It also checks canvas double-click fullscreen entry/exit in all four display
modes, menu-action labels, excluded clicks, and F11 after using the gesture.
It does not operate the user's desktop or save their preferences.

Build with `cargo test --release --no-run`, then set `SONGREC_TEST_BINARY` to the
test executable printed by Cargo (`target/release/deps/songrec-<hash>`). Run:

```sh
env GIO_USE_VFS=local GDK_DEBUG=no-portals GTK_A11Y=none \
  dbus-run-session -- mutter --headless --wayland \
  --virtual-monitor=1280x1024 --wayland-display=songrec-regression -- \
  env SONGREC_TEST_HEADLESS=1 GDK_BACKEND=x11 GSK_RENDERER=cairo \
  "$SONGREC_TEST_BINARY" real_pointer_dismisses_menu_after_using_nested_dropdowns \
  --ignored --nocapture --test-threads=1
```

Repeat with `GDK_BACKEND=wayland`. X11 uses `xdotool` only for window geometry
and activation; input uses Mutter's private RemoteDesktop session. Wayland uses
a maximized, undecorated non-fullscreen window so coordinates are known without
depending on an unavailable global-position API. Both backends use undecorated
windows. The Wayland item-selection cases use real arrow/Enter keys because the
tested GTK/Mutter combination reports inconsistent nested-popup parent positions;
opening and toggle-closing the dropdown and outside dismissal still use real
pointer input. Selecting an item with the mouse on Wayland remains a manual
check. The compositor exits with the
test. Run these sessions sequentially and only set `SONGREC_TEST_HEADLESS=1`
inside this private desktop.

The settings lifecycle test loads the real preferences widgets and context-menu
controls, checks both synchronization directions, changes two sliders immediately
before `Application::quit`, and checks the production shutdown hook's saved
snapshot and freshly constructed controls. It uses a temporary preferences file;
unrelated main-window builder callbacks are no-ops. Use its filter in the same
private-desktop command above.

## Artwork and notifications

- The acquisition tests inject a slow preferred request and a fast original;
  they do not contact Shazam or Apple.
- The mapped artwork test injects a newer missing cover during active transitions
  in Cinema and Ambient and verifies the already-ready scene remains visible.
- Desktop notifications wait for artwork success/failure before sending once;
  a 6.5-second backstop sends text if a completion is missing. Recognition in the
  main window is not delayed by this notification policy.
- Classic prepares the original and palette only. Entering Cinema/Ambient lazily
  adds the shared blurred texture. Motion values and paths are unchanged.

The notification protocol regression uses a mock Freedesktop notification daemon
on a **private** session bus (never run its marker on the desktop session bus):

```sh
env GIO_USE_VFS=local dbus-run-session -- \
  env SONGREC_TEST_PRIVATE_BUS=1 GNOTIFICATION_BACKEND=freedesktop \
  "$SONGREC_TEST_BINARY" freedesktop_notification_delivers_readable_artwork_once \
  --ignored --nocapture --test-threads=1
```

It verifies that the daemon receives readable original encoded artwork via a
file icon, that unavailable artwork sends text only, that disabling notifications
cancels a queued icon write's delivery, and that repeated artwork completions do
not duplicate a notification. The current temporary icon is retained until
replacement or application shutdown. This is protocol coverage, not a visual
test of every desktop's notification daemon.

## Reproducible processing measurements

Run `artwork_pipeline_stage_timings` with `--ignored --nocapture --test-threads=1`
on the release test executable, without another build running. It measures
generated 400×400 and 1600×1600 JPEG/PNG fixtures, excluding fixture encoding and
network time. Each pipeline result is the median of eight runs after one warm-up.
Texture creation is measured separately from worker preparation; retained-payload
bytes are **not** process RSS or GPU usage. No timing threshold is asserted.

On the development machine, the 1600×1600 preparation path initially took about
172 ms, versus about 5 ms for Classic's palette-only path and 0.02 ms for creating
and upgrading GTK memory textures. The stage probe identified tone processing
as a substantial cost. Skipping the disabled vignette work and replacing costly
per-channel rounding with equivalent byte rounding reduced immersive preparation
to about 133 ms (roughly 23%). Resolution, blur, colors, and motion parameters are
unchanged. Rounding-boundary and existing image-treatment regressions cover the
output; these timings do not claim a 23% improvement in network or frame latency.

## JSON response → final high-resolution artwork

To measure normal song changes in the actual application, run:

```sh
SONGREC_ARTWORK_TIMING=1 cargo run --release -- -v
```

Keep Now Playing visible while songs change. `Artwork latency:` reports the time
from receipt of the **parsed recognition JSON**, before track metadata extraction,
to the first frame with the matching prepared artwork and a fully revealed
transition. It includes download/fallback, decoding, worker scheduling, background
preparation, GUI message handling, texture application, rendering, and the selected
transition. It excludes microphone capture, fingerprinting, the Shazam request,
and JSON body decoding itself. The source dimensions are decoded dimensions,
not dimensions assumed from the requested URL.

The endpoint is GDK's actual compositor presentation timestamp when available.
If feedback is absent/incomplete after 500 ms, the log explicitly reports
the **GTK after-paint** endpoint, not presentation or confirmed GPU submission.
That feedback wait only delays the log;
it is not added to the measured latency or to rendering. Predicted presentation
times are never used. Presentation feedback is not a measurement of physical
monitor scanout. Logs are emitted once per displayed response, not every frame;
hidden-window responses, unavailable artwork, Lights Off, and unchanged results
that never replace the scene do not produce samples. Disable the environment
variable to remove the frame observer entirely.

The accompanying `-v` stage logs identify the track and show download, decoding,
background worker queue/processing, texture construction, scene application, and
transition completion. `response age` at GUI receipt and scene application uses
the same origin. A cache hit bypasses download/decode and, when prepared textures
are cached, background preparation. Scene application is **not** a displayed
frame; a slide also applies the new scene only after its outgoing half finishes.

### Live CDN replay

Build with `cargo test --release --no-run` and set `SONGREC_TEST_BINARY` as above.
The ignored `live_response_to_final_artwork_frame` test makes six public Apple CDN
downloads and performs 24 song changes in a private fullscreen window. It replays
recognition-shaped JSON through the production parser, acquisition service,
recognition state, and Now Playing update/preparation/transition code. It does
**not** send Shazam requests, record audio, or save preferences.

```sh
env GIO_USE_VFS=local GDK_DEBUG=no-portals GTK_A11Y=none \
  dbus-run-session -- mutter --headless --wayland \
  --virtual-monitor=1920x1080 --wayland-display=songrec-artwork-timing -- \
  env SONGREC_TEST_LIVE_ARTWORK=1 GDK_BACKEND=wayland GSK_RENDERER=gl \
  "$SONGREC_TEST_BINARY" live_response_to_final_artwork_frame \
  --ignored --nocapture --test-threads=1
```

This opt-in test requires network access; it fails rather than treating an
unavailable image or thumbnail fallback as a high-resolution timing. A requested
1600px rendition can legitimately be capped at a smaller native source size.
Each mode starts with a displayed synthetic track and a fresh acquisition and
preparation cache. The first two real albums are application-cache cold; later
changes alternate those albums with warm caches. **CDN and OS caches are not
controlled.** Run without another build/benchmark consuming CPU/GPU resources.

Measured on 2026-09-12, release build, private Mutter 50.4 Wayland display,
1920×1080 canvas, GskGLRenderer on i915. Rumours returned 1600×1600 (353,290 bytes);
The Wall returned 1414×1414 (269,843 bytes). These are individual observations /
two-album ranges, not percentiles or guarantees for arbitrary artwork or networks:

| Mode | Cold, no transition (Rumours) | Cold, 2 s crossfade (The Wall) | Warm, no transition | Warm, 2 s crossfade / slide |
| --- | ---: | ---: | ---: | ---: |
| Classic | 306 ms | 2,082 ms | 24–35 ms | 2,020–2,034 ms* |
| Cinema | 405 ms | 2,315 ms | 34–55 ms | 2,017–2,067 ms* |
| Ambient | 396 ms | 2,273 ms | 33–36 ms | 2,010–2,059 ms* |

\* Some animated final frames lacked presentation feedback and use the explicitly
labelled after-paint endpoint. All cold samples and warm non-animated samples in
this run had compositor presentation feedback.

Across the six cold acquisitions, network transfer took 37–199 ms and decode
(including worker dispatch) 22–55 ms. Classic background preparation took 5–8 ms;
immersive preparation took 140–156 ms. Worker queue times were below 1 ms and GTK
memory-texture construction below 0.03 ms (GPU work occurs later). Cached changes
skipped both acquisition and preparation. The configured two-second transition
was the dominant part of the animated total; settings and processing output were
not changed to improve these numbers.

The replay uses the real Now Playing path but not the main window's additional
history, notification, MPRIS, and audio workload. It therefore does not rule out
contention in a normal application run or slow CDN/fallback responses. Use the
opt-in application trace above to investigate those cases before changing quality,
download budgets, or transition behavior.

`artwork_timing_waits_for_ready_scene_and_cleans_up` is a separate, network-free
ignored GTK regression. It injects delayed artwork, checks that samples wait for
artwork and the full crossfade/slide, rejects repeat samples on repaint/remap,
and verifies that dropping the probe releases its callbacks. Run its filter in
the same private display command, without `SONGREC_TEST_LIVE_ARTWORK=1`.
