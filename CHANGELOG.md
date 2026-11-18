# Changelog

## [2.0.15] - 2026-11-18

### Added

- Give the agent an environment map of the host, built once and answered from its own tools:
  `env_scan` walks a root in two passes (one `find` for files with size and age, one `grep` for
  declarations) and stores the result, `env_search` ranks a phrase against the mapped files, and
  `env_find`, `env_outline` and `env_overview` answer from the map without touching the host, so
  "where is this handled?" no longer costs a grep per question. `env_system` records the operating
  system, the listening sockets and the running services, and answers from what it holds
  afterwards. The map lives in the host's plugin data, survives a restart, is shared by every chat
  on that connection and is invalidated path by path as the agent writes.
- Loaded scopes are quiet neutral tags under a "Loaded scopes:" caption over the composer, with a
  tooltip saying a click removes the scope from the context. A click opens the scope's details —
  its summary, how many tools it carries, their names, and a "Turn off" action — instead of
  dropping it straight away.
- The AI chat list works like the reference: a chat shows only a back button and its own title in
  place of the title in the pane header, and the back button files the settled chat and shows the
  list newest first. The list shares the composer, where a typed prompt starts and enters a new
  chat while a run in flight stays in its own conversation.
- Draw the interface in the platform's own UI face: the window installs the system font first in
  the proportional family and keeps egui's own behind it for the glyphs a system face lacks, so
  labels, dialogs and chat text belong to the desktop instead of to the bundled font.
- Options and a host's Appearance offer the installed font families in a dropdown, and the terminal
  draws, measures and caches its cells in the family a session names instead of egui's monospace.
  Registration happens in the frame loop, so a family chosen in Options reaches the tabs already
  open.

### Changed

- Steer the agent to the file tools instead of the shell: the scope index names the `dev_*` tools
  the host scope holds, `run_command` says what it is not for, and the guidelines lead with tools
  over `cat`, `grep` and `sed` and name the scope to load when the file tools are not loaded yet.
- The AI pane's back button is drawn at its own size in the pane's colours, instead of a glyph that
  came out too small in whatever colour the font gave it.
- OpenAI-compatible providers get a reply's `reasoning_content` echoed back with the assistant turn,
  so a model that reasons before answering keeps that context across tool calls.

## [2.0.14] - 2026-11-18

### Added

- Give the AI agent's tools a graph of scopes the model walks on demand, instead of sending the
  schemas of all 48 tools on every request. A chat starts with the commands themselves plus two
  new tools, `load_scope` and `unload_scope`; a scope is loaded when the model needs it and can be
  dropped again.
  - The graph nests — `host` holds files, system statistics and the terminal — and carries "see
    also" edges, so a scope related to a loaded one can be loaded without walking through its
    parent. An edge only widens what may be loaded next and never loads anything itself.
  - Only what a loaded scope holds is sent, only the scopes this host may use are offered at all,
    and a scope whose capability is switched off in the host settings never appears.
  - What a conversation loaded travels with it: the live chat and every chat in the history keep
    their own set, and a reload restores it.
  - The pane shows the loaded scopes as chips over the composer; a click unloads one, along with
    everything inside it, and frees the room its schemas took in the prompt.
  - A call to a tool from a scope that is not loaded is answered with the scope to load rather
    than being run.

### Changed

- The AI agent's prompt names the loaded scopes and their tools and lists what can be loaded
  next, instead of describing every tool on every turn. The guidance for a family of tools
  (files, terminal, shells) appears once that family is loaded, so a switched-off capability is
  never named.
- Auto-compaction measures the request that is actually sent — the loaded schemas and the prompt
  — instead of reserving a fixed 4 000 tokens for the whole toolset, giving a conversation that
  carries one or two scopes its room back.

## [2.0.13] - 2026-11-16

### Added

- Give every shell card in the AI chat two copy buttons: one for the command it ran, and one
  level with the first line of its output for everything it printed.

### Fixed

- Create groups from a dialog with Cancel and Create, and keep a group on screen — and open to a
  dragged host — while it is still empty.
- Keep the colour picked for a group instead of resetting it to the stored one.
- Select the first character of a line: dragging a selection back towards column 0 used to drop
  the cell under the pointer.

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