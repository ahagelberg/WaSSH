# Changelog

## [2.0.12] - 2026-11-16

### Changed

- Set the AI agent chat's text with more room between the lines: body text, list items,
  quotes and fenced code all move from egui's default line height to 1.4× the font size.

### Fixed

- Stop the panes from rebuilding what has not changed on every frame. Each of these now
  works from a cached result and refreshes only when its input changes:
  - the file browser's table sorts and formats its rows per listing, instead of formatting
    and measuring every entry and lowercasing names inside the sort comparator on each frame;
  - the Docker pane's container list applies its search and ordering when the list or the
    search changes, instead of cloning every container and sorting it every frame;
  - the Connection Log's timeline, step chart and heatmap are folded from the log once per
    log change or second, instead of walking every entry on each frame;
  - the AI agent parses each message's Markdown once and keeps it until that message's text
    changes, so a long chat no longer re-parses its whole transcript per frame;
  - the AI agent's header caption is built from the run state, the session state and the
    first question, instead of copying the first message every frame;
  - the AI agent's context budget keeps the conversation's size with the change it was
    counted at, instead of adding up every message on each frame;
  - the Server Monitor orders and formats the process table per sample and per sort column,
    instead of sorting and formatting all eight cells of every row on each frame;
  - command rules are compiled once per pattern, instead of once per rule per tool call;
  - the terminal draws each style run into one reused buffer, instead of allocating a string
    for every run of every row on each frame;
  - the settings dialogs borrow the host list instead of cloning every host per frame.

## [2.0.11] - 2026-11-16

### Added

- Compact a long AI agent chat: once the conversation would outgrow the model's context
  window, the older messages are summarised and dropped, and the summary is carried in the
  prompt. Each model's window is a row in Options — 128 000 tokens for models without one —
  and automatic compaction can be switched off.

## [2.0.10] - 2026-11-13

### Added

- Ask the AI agent's questions one at a time: picking an option answers and moves on, Previous
  walks back over the answers, and the text field is always there as the free-form answer.
- Fill the chat's user messages with the accent colour and keep the newest one pinned above the
  transcript while a long reply scrolls.

### Fixed

- Lay the Connection Log out by hand, so its cards share one row and the timeline, step chart or
  heatmap is drawn inside a docked pane instead of below it.

## [2.0.9] - 2026-11-12

### Fixed

- Install the Windows build for every user, in Program Files, instead of into one
  user's profile. Older copies in the profile are removed by the installer.

## [2.0.8] - 2026-11-12

### Added

- Publish an AppImage with the Linux release, next to the tarball.
- Publish Debian and RPM packages with the Linux release.

### Fixed

- Stop Windows from opening a console window for every command the app runs behind
  the scenes.

## [2.0.7] - 2026-11-11

### Added

- Redraw the Docker pane as cards, with right-click menus for containers, images,
  volumes and networks.
- Open a container's details, logs and stats inside its own card.
- Create containers in a dialog with Docker Hub and Quay search, restart policies and
  renaming.
- Explain why Docker cannot be reached and offer the install, start or group fix.
- Add local PowerShell, cmd and WSL terminals, and let a WSL tab's files be the
  distribution's files.
- Add a bottom status bar with the active session state and session counts.
- Publish each pane's tools to the AI agent, and tell it which environment the session
  runs in.
- Show the program icon in the title bar.

### Fixed

- Keep an SSH command's exit status when it arrives after EOF, so a Docker failure
  reports its reason instead of an empty answer.
- Keep long AI agent chat cards inside the chat pane.
- Type the sudo password into the Docker password card without an extra newline, and
  focus the field when it opens.
- Stop popup dialogs from forwarding keystrokes to the terminal.
- Build the Windows release as a GUI app, and stop warning about Windows-only code on
  Linux.

## [2.0.6] - 2026-11-10

### Added

- Share SSH identities and encrypted credentials across hosts and sessions.
- Import configuration and available credentials from the legacy Electron app.

### Notes

- Legacy encrypted credentials can be imported on Windows and from Linux Secret Service;
	other encrypted backends require credentials to be entered again.

## [2.0.5] - 2026-11-10

### Added

- Publish a Windows installer alongside the portable ZIP.
- Save the local terminal as a host for convenient access.

### Fixed

- Improve Markdown rendering in the AI agent.
- Preserve composer focus during AI agent questions.
- Improve AI agent approval and output cards.

### Notes

- The native app does not yet support automatic updates or migration of Electron settings.

## [2.0.4] - 2026-11-06

### Added

- Add Plain, Markdown, and Hex display modes to the file viewer.
- Show scan progress and file counts while downloading folders and creating ZIP archives.
- Add Stop controls for transfers, copies, and ZIP operations.

### Fixed

- Keep transfer progress independent from folder browsing and show copy batches in the transfer list.
- Reduce SSH directory scan requests by reusing entry type and size metadata.
- Make file viewer content taller and render Markdown files with formatted text.
- Handle empty folders and links safely while creating ZIP archives.