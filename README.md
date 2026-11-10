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
| `wassh-sys` | Native facts about this machine through the OS (Win32 on Windows, `/proc` on Linux): CPU, memory, disk, network, processes |
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

- On startup, WaSSH imports pre-2.0 Electron configuration from the sibling `WaSSH`
  data directory once, leaving the original files untouched. Existing native settings,
  tabs and plugin files take precedence; hosts and known hosts are merged without
  replacing existing entries. Saved passwords, SSH key passphrases and AI provider
  keys are imported when possible, without replacing existing native credentials.
  SSH private key paths are retained; key files remain in their original locations.
  Encrypted vaults are supported on Windows under the original user account and on
  Linux with an unlocked Secret Service keyring. Unlock the keyring and restart to
  retry unavailable credentials. Other encrypted backends, including KWallet and
  macOS Keychain, require credentials to be entered again. The old vault is retained.
- Long-running builds: `CARGO_BUILD_JOBS=8 taskset -c 0-9 cargo build` (this machine is
  limited to 10 cores and must not run parallel builds).
- `wassh --spike` runs the terminal/SSH spike harness; `--widgets` shows the widget gallery.
- `vendor/winit` is winit 0.30.13 with the Windows 11 mixed-DPI fix backported (see
  `vendor/winit/PATCHED.md`). Drop the directory and the `[patch.crates-io]` entry once
  eframe moves to a winit release that contains it.
