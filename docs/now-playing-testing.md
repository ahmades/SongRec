# Now Playing regression checks

Run the non-display suite with `cargo test --release --all-targets`.

The ignored GTK tests require a GTK4 display. Run each filter in its own process
(GTK initialization is thread-affine), with `--ignored --test-threads=1`:

- `settings_controls_are_labelled_and_slider_gestures_do_not_retain_widgets`
- `deferred_artwork_resumes_and_transitions_with_metadata`
- `artwork_layout_callbacks_do_not_retain_containers`
- `mapped_transitions_retain_crossfade_scene_and_bound_slide_allocations`

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

Run with `-v` to include the menu pointer/close diagnostics. Keep backend details
and the exact dropdown open/close sequence with any failure report.

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
