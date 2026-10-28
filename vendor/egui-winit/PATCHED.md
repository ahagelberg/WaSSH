# Vendored egui-winit 0.37.0 (patched)

Unmodified upstream source except for the Insert fix below. Do not edit anything else
here; when an egui-winit release stops claiming Insert, delete this directory and the
`[patch.crates-io]` entry in `../Cargo.toml`.

## Patch: leave Ctrl+Insert and Shift+Insert to the application

Upstream `is_copy_command` / `is_paste_command` translate these into
`egui::Event::Copy` / `egui::Event::Paste` on Windows, and the copy rule does not exclude
Shift, so Ctrl+Insert *and* Ctrl+Shift+Insert both became `Event::Copy`. A terminal has to
send `^C` for `Event::Copy`, so both keys interrupted the shell instead of copying, and
the two presses could not be told apart afterwards.

`NativeOptions::clipboard_shortcuts = false` would fix it, but it also drops the
Cmd/Ctrl+C/X/V translation that `egui::TextEdit` relies on. With the two Insert terms
removed, C/X/V behave exactly as before and the application sees Insert as an ordinary
key, which `Shell::shortcuts` binds to copy and paste.

Files changed:

- `src/lib.rs`: `is_copy_command` and `is_paste_command` only match `Key::Copy` /
  `Key::Paste` and the Cmd/Ctrl+C and Cmd/Ctrl+V chords.
