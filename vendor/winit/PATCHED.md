# Vendored winit 0.30.13 (patched)

Unmodified upstream source except for the Windows mixed-DPI fix below. Do not edit
anything else here; when a winit release contains the fix (0.31 already does), delete
this directory and the `[patch.crates-io]` entry in `../Cargo.toml`.

## Patch: apply the OS-suggested rect on Windows 11

Upstream bugs: [winit#4041](https://github.com/rust-windowing/winit/issues/4041),
[winit#4600](https://github.com/rust-windowing/winit/issues/4600),
[egui#7648](https://github.com/emilk/egui/issues/7648).

`WM_DPICHANGED` derived a "conservative" rect and then nudged it, pixel by pixel, onto
the monitor returned by `MonitorFromWindow`. At the time the message arrives the window
has not moved yet, so that monitor is the one being left, and the nudge drags the window
back onto it. The move produces another `WM_DPICHANGED`, which scales the window by the
DPI ratio again: dragging a window across a monitor boundary makes it jump around and
grow on every crossing.

Files changed (backport of [winit#4341](https://github.com/rust-windowing/winit/pull/4341)):

- `src/platform_impl/windows/util.rs`: added `WIN10_BUILD_VERSION` (read through
  `RtlGetVersion`, because `GetVersionEx` lies outside a manifest).
- `src/platform_impl/windows/event_loop.rs`: the conservative-rect + nudge path now only
  runs on Windows 10 (build < 22000). On Windows 11+ the rect Windows suggests in
  `WM_DPICHANGED` is applied as is, which keeps the logical size and the cursor anchor.

Verified with `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo test --workspace` on Windows 11.
