# WaSSH

Native WaSSH desktop client, built as a single self-contained Cargo workspace.

## Layout

| Crate | Responsibility |
|---|---|
| `wassh-core` | Domain model: app constants, palette/theme, session status + backoff, dock layout |
| `wassh-store` | Config persistence (settings, hosts, tabs, known hosts, plugin data) |
| `wassh-ssh` | SSH transport: auth, jump chains, channels, tunnels, X11, SFTP |
| `wassh-term` | Terminal emulation glue (alacritty_terminal), input encoding, selection/search |
| `wassh-term-render` | Glyph atlas and cell mesh generation |
| `wassh-sys` | Native facts about this machine through the Win32 API (CPU, memory, disk, network, processes) |
| `wassh-ui` | egui shell, widget kit, screens |
| `wassh-plugins` | Internal plugin trait and the eight built-in plugins |
| `wassh` | Application binary |

## Commands

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo run -p wassh
```

## Notes

- Long-running builds: `CARGO_BUILD_JOBS=8 taskset -c 0-9 cargo build` (this machine is
  limited to 10 cores and must not run parallel builds).
- `wassh --spike` runs the terminal/SSH spike harness; `--widgets` shows the widget gallery.
- `vendor/winit` is winit 0.30.13 with the Windows 11 mixed-DPI fix backported (see
  `vendor/winit/PATCHED.md`). Drop the directory and the `[patch.crates-io]` entry once
  eframe moves to a winit release that contains it.
