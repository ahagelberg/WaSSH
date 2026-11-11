# Changelog

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