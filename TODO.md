## Basic ##
- [x] Connection logging: check for network instability - with graph/timeline

## Advanced ##
- Key manager for setting up password-less logins
- Tunnel tool to create tunnels for multiple ports through multiple layers of machines
- Broadcast commands to multiple machines at ones. Collect output. Allow diff between machines.
- Topology view (plugin) built up autoamtically based on connections, tunnels, hops etc.
- Workspaces
- Tunnels guide: pre-defined common tunnels as templates.
- Injectable server that can run things even when not connected
    - scheduled tasks
    - network and other resource monitoring

## Nice to have ##
- Share settings easily
- Overview of all open sessions with thumbnails
- Global command history with quick search
- Detection of server capability, services running. custom detection commands.

## Plugins ##
- Octopy-plugin:
    - shows sub-tabs for each screen (possibly with different username)
    - show node info
    - show Ink in browser window?
- Monitor all hosts - including not connected
- SCP/SFTP plugin: add edit function (text files using builtin editor) etc.
- SCP/SFTP plugin: Allow tansfer between hosts?
- MQTT plugin: Add history for published messages.
- MQTT plugin: Add option to have a list of common messages for publishing
- MQTT plugin: add basic triggers for activating commands and other things based on messagesw received
- Add MCP capability to plugins - AI Agent can access MQTT and other things.
- AI Agent: Add option to include (yaml) files with prompts
- [x] AI Agent: tool access to file system (remote SFTP and local client PC)
- [x] AI Agent: web search and web access support (Bing default + Brave/Google/DuckDuckGo/Custom)
- [x] AI Agent: date_time access
- [x] SFTP plugin: Allow download entire folder as zip file
- [x] AI Agent: ollama bug - doesn't show all models available (added a refresh button next to the model dropdown that fetches all models currently available from the provider, e.g. everything pulled into a local Ollama server, and merges them into the selectable list; also gave Ollama its own dedicated provider preset instead of a generic "Local" one)
- [x] AI Agent: Allow the prompt input box to grow when more text is input