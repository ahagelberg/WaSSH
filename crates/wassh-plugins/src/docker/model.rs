//! What the Docker pane shows, and the `docker` commands that produce it.
//!
//! Port of the reference's `dockerClient.ts` and `protocol.ts`. The reference
//! spoke the Engine HTTP API over the daemon socket; this pane asks the `docker`
//! CLI the same questions through the session's command channel, so it needs no
//! socket transport of its own. Everything the CLI is asked for is machine
//! readable (`--format '{{json .}}'` or `inspect` JSON), so the parsers here see
//! the same fields the API returned.

use serde_json::Value;

/// The daemon socket the reference defaults to.
pub const DEFAULT_SOCKET: &str = "/var/run/docker.sock";
/// Lines of log fetched when the log panel opens, unless the host says otherwise.
pub const DEFAULT_LOG_TAIL: usize = 200;
/// Container a shell is offered in.
pub const SHELL_CHOICES: [&str; 6] = ["/bin/sh", "/bin/bash", "sh", "bash", "ash", "zsh"];

/// A published port.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Port {
    /// Address the port is published on, when docker reports one.
    pub host_ip: String,
    /// Port on the host.
    pub host_port: Option<u16>,
    /// Port inside the container.
    pub container_port: u16,
    /// `tcp` or `udp`.
    pub protocol: String,
}

impl Port {
    /// `8080→80/tcp` as shown in the table.
    pub fn label(&self) -> String {
        match self.host_port {
            Some(host) => format!("{host}→{}/{}", self.container_port, self.protocol),
            None => format!("{}/{}", self.container_port, self.protocol),
        }
    }
}

/// A bind mount or named volume.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Mount {
    /// `bind` or `volume`.
    pub kind: String,
    /// Host path or volume name.
    pub source: String,
    /// Path inside the container.
    pub destination: String,
    /// Whether the container may write to it.
    pub writable: bool,
}

/// One container, as the list shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Container {
    /// Full id.
    pub id: String,
    /// Name without the leading slash.
    pub name: String,
    /// Image reference it was created from.
    pub image: String,
    /// Command line.
    pub command: String,
    /// `running`, `exited`, `paused`, `created`, `restarting`, `dead`…
    pub state: String,
    /// Human status line: "Up 3 hours", "Exited (1) 2 days ago".
    pub status: String,
    /// Published ports.
    pub ports: Vec<Port>,
    /// Mounts.
    pub mounts: Vec<Mount>,
    /// Networks it is attached to.
    pub networks: Vec<String>,
    /// Labels, as key/value pairs in docker's order.
    pub labels: Vec<(String, String)>,
}

impl Container {
    /// Whether the container is up.
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }

    /// Whether it is paused (which still counts as running).
    pub fn is_paused(&self) -> bool {
        self.state == "paused"
    }

    /// First twelve characters of the id, for dense rows.
    pub fn short_id(&self) -> &str {
        short_id(&self.id)
    }

    /// Actions that make sense for this container, in the reference's order.
    pub fn actions(&self) -> Vec<Action> {
        match self.state.as_str() {
            "running" => vec![
                Action::Stop,
                Action::Restart,
                Action::Pause,
                Action::Kill,
                Action::Remove,
            ],
            "paused" => vec![Action::Unpause, Action::Stop, Action::Kill, Action::Remove],
            "restarting" => vec![Action::Stop, Action::Kill, Action::Remove],
            _ => vec![Action::Start, Action::Remove],
        }
    }

    /// Where the container sits in the reference's list order: running first.
    pub fn rank(&self) -> u8 {
        match self.state.as_str() {
            "running" => 0,
            "paused" => 1,
            "restarting" => 2,
            _ => 3,
        }
    }
}

/// What can be done to a container.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Restart,
    Pause,
    Unpause,
    Kill,
    Remove,
}

impl Action {
    /// The docker subcommand.
    pub fn verb(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
            Self::Pause => "pause",
            Self::Unpause => "unpause",
            Self::Kill => "kill",
            Self::Remove => "rm",
        }
    }

    /// Wording for the status line and the confirmation.
    pub fn done(self) -> &'static str {
        match self {
            Self::Start => "Started",
            Self::Stop => "Stopped",
            Self::Restart => "Restarted",
            Self::Pause => "Paused",
            Self::Unpause => "Resumed",
            Self::Kill => "Killed",
            Self::Remove => "Removed",
        }
    }

    /// Whether the action cannot be undone, so the settings can gate it.
    pub fn is_destructive(self) -> bool {
        matches!(self, Self::Kill | Self::Remove)
    }
}

/// One image.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Image {
    /// Image id, without the `sha256:` prefix.
    pub id: String,
    /// `repository:tag` references that point at it.
    pub references: Vec<String>,
    /// When it was built, as docker prints it.
    pub created: String,
    /// Size on disk in bytes.
    pub size: u64,
    /// Containers this image is used by, counted from the container list.
    pub containers: usize,
}

/// One volume.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Volume {
    /// Volume name.
    pub name: String,
    /// Driver, usually `local`.
    pub driver: String,
    /// Scope, `local` or `global`.
    pub scope: String,
    /// Where it lives on the host.
    pub mountpoint: String,
}

/// One network.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Network {
    /// Full id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Driver, usually `bridge`.
    pub driver: String,
    /// Scope.
    pub scope: String,
    /// Whether it is internal-only.
    pub internal: bool,
}

impl Network {
    /// First twelve characters of the id.
    pub fn short_id(&self) -> &str {
        short_id(&self.id)
    }
}

/// One stats sample.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    /// Share of one CPU, as docker reports it (can exceed 100 on many cores).
    pub cpu_percent: f64,
    /// Memory in use.
    pub memory_used: u64,
    /// Memory the container may use.
    pub memory_limit: u64,
    /// Share of the limit in use.
    pub memory_percent: f64,
    /// Threads and processes.
    pub pids: u64,
    /// Bytes received.
    pub net_rx: u64,
    /// Bytes sent.
    pub net_tx: u64,
    /// Bytes read from disk.
    pub block_read: u64,
    /// Bytes written to disk.
    pub block_write: u64,
}

/// A container's inspect output, in the parts the detail panel shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Detail {
    /// Full id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Image reference.
    pub image: String,
    /// When it was created.
    pub created: String,
    /// Status line.
    pub status: String,
    /// When it started, or an empty string.
    pub started_at: String,
    /// When it exited, or an empty string.
    pub finished_at: String,
    /// Exit code, when it has exited.
    pub exit_code: Option<i64>,
    /// Process id on the host, when running.
    pub pid: Option<i64>,
    /// Restarts so far.
    pub restart_count: i64,
    /// Platform, e.g. `linux`.
    pub platform: String,
    /// Entrypoint and command as one line.
    pub command: String,
    /// Working directory.
    pub workdir: String,
    /// User the process runs as.
    pub user: String,
    /// Whether a TTY is allocated.
    pub tty: bool,
    /// Restart policy name.
    pub restart_policy: String,
    /// Network mode.
    pub network_mode: String,
    /// Whether the container is privileged.
    pub privileged: bool,
    /// Whether it deletes itself when it exits.
    pub auto_remove: bool,
    /// Environment variables.
    pub env: Vec<String>,
    /// Port bindings as `host:container/proto` lines.
    pub ports: Vec<String>,
    /// Networks with their addresses.
    pub networks: Vec<String>,
    /// Mounts as `source → destination (mode)` lines.
    pub mounts: Vec<String>,
    /// The whole inspect document, pretty-printed.
    pub raw: String,
}

/// A container to create, as the create form fills it in.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunRequest {
    /// Image reference without the tag.
    pub image: String,
    /// Tag; empty means `latest`.
    pub tag: String,
    /// Container name; empty lets docker pick one.
    pub name: String,
    /// Overrides the image's command.
    pub command: String,
    /// `host:container[/proto]` port mappings.
    pub ports: Vec<String>,
    /// `KEY=VALUE` environment entries.
    pub env: Vec<String>,
    /// `source:destination[:ro]` mounts.
    pub volumes: Vec<String>,
    /// Restart policy: `no`, `always`, `unless-stopped`, `on-failure`.
    pub restart_policy: String,
    /// Network to attach to.
    pub network: String,
    /// Run detached (the default) rather than in the foreground.
    pub detach: bool,
    /// Delete the container when it exits.
    pub auto_remove: bool,
}

impl RunRequest {
    /// `repository:tag`.
    pub fn reference(&self) -> String {
        let tag = self.tag.trim();
        if tag.is_empty() {
            format!("{}:latest", self.image.trim())
        } else {
            format!("{}:{tag}", self.image.trim())
        }
    }
}

/// Single-quote a value for a POSIX shell.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// First twelve characters of an id.
pub fn short_id(id: &str) -> &str {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    &id[..id.len().min(12)]
}

/// Ids and names are pasted into a shell command, so only accept what docker emits.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/' | '@'))
}

/// `-H unix://…` when the host uses a socket other than the default.
fn socket_flag(socket: &str) -> String {
    let socket = socket.trim();
    if socket.is_empty() || socket == DEFAULT_SOCKET {
        return String::new();
    }
    format!(" -H {}", shell_quote(&format!("unix://{socket}")))
}

/// The container list, one JSON object per line.
pub fn ps_command(socket: &str, all: bool) -> String {
    let mut command = format!("docker{} ps", socket_flag(socket));
    if all {
        command.push_str(" -a");
    }
    command.push_str(" --no-trunc --format '{{json .}}'");
    command
}

/// `docker inspect` for one container, as a JSON array.
pub fn inspect_command(socket: &str, id: &str) -> Option<String> {
    is_safe_name(id).then(|| format!("docker{} inspect {}", socket_flag(socket), shell_quote(id)))
}

/// One stats sample for one container.
pub fn stats_command(socket: &str, id: &str) -> Option<String> {
    is_safe_name(id).then(|| {
        format!(
            "docker{} stats --no-stream --format '{{{{json .}}}}' {}",
            socket_flag(socket),
            shell_quote(id)
        )
    })
}

/// The log tail for one container, with stderr folded in.
pub fn logs_command(socket: &str, id: &str, tail: usize) -> Option<String> {
    is_safe_name(id).then(|| {
        format!(
            "docker{} logs --tail {tail} {} 2>&1",
            socket_flag(socket),
            shell_quote(id)
        )
    })
}

/// A lifecycle command for one container.
pub fn action_command(socket: &str, id: &str, action: Action) -> Option<String> {
    if !is_safe_name(id) {
        return None;
    }
    let verb = action.verb();
    let flag = match action {
        Action::Stop | Action::Restart => " -t 10",
        Action::Remove => " -f",
        _ => "",
    };
    Some(format!(
        "docker{} {verb}{flag} {}",
        socket_flag(socket),
        shell_quote(id)
    ))
}

/// The image list, one JSON object per line.
pub fn images_command(socket: &str) -> String {
    format!(
        "docker{} images --no-trunc --format '{{{{json .}}}}'",
        socket_flag(socket)
    )
}

/// Pull an image.
pub fn pull_command(socket: &str, reference: &str) -> String {
    format!(
        "docker{} pull {}",
        socket_flag(socket),
        shell_quote(reference.trim())
    )
}

/// Remove an image.
pub fn remove_image_command(socket: &str, id: &str, force: bool) -> Option<String> {
    is_safe_name(id).then(|| {
        format!(
            "docker{} rmi{} {}",
            socket_flag(socket),
            if force { " -f" } else { "" },
            shell_quote(id)
        )
    })
}

/// The volume list, one JSON object per line.
pub fn volumes_command(socket: &str) -> String {
    format!(
        "docker{} volume ls --format '{{{{json .}}}}'",
        socket_flag(socket)
    )
}

/// Create a volume.
pub fn create_volume_command(socket: &str, name: &str) -> Option<String> {
    is_safe_name(name).then(|| {
        format!(
            "docker{} volume create {}",
            socket_flag(socket),
            shell_quote(name)
        )
    })
}

/// Remove a volume.
pub fn remove_volume_command(socket: &str, name: &str, force: bool) -> Option<String> {
    is_safe_name(name).then(|| {
        format!(
            "docker{} volume rm{} {}",
            socket_flag(socket),
            if force { " -f" } else { "" },
            shell_quote(name)
        )
    })
}

/// Remove every volume nothing uses.
pub fn prune_volumes_command(socket: &str) -> String {
    format!(
        "docker{} volume prune -f --format '{{{{json .}}}}'",
        socket_flag(socket)
    )
}

/// The network list, one JSON object per line.
pub fn networks_command(socket: &str) -> String {
    format!(
        "docker{} network ls --format '{{{{json .}}}}'",
        socket_flag(socket)
    )
}

/// Create a network.
pub fn create_network_command(socket: &str, name: &str, driver: &str) -> Option<String> {
    if !is_safe_name(name) || !is_safe_name(driver) {
        return None;
    }
    Some(format!(
        "docker{} network create --driver {} {}",
        socket_flag(socket),
        shell_quote(driver),
        shell_quote(name)
    ))
}

/// Remove a network.
pub fn remove_network_command(socket: &str, id: &str) -> Option<String> {
    is_safe_name(id).then(|| {
        format!(
            "docker{} network rm {}",
            socket_flag(socket),
            shell_quote(id)
        )
    })
}

/// Create and start a container.
pub fn run_command(socket: &str, request: &RunRequest) -> String {
    let mut command = format!("docker{} run", socket_flag(socket));
    command.push_str(" -d");
    if request.auto_remove {
        command.push_str(" --rm");
    }
    if let Some(policy) = restart_policy_flag(&request.restart_policy) {
        command.push_str(" --restart ");
        command.push_str(policy);
    }
    if is_safe_name(request.network.trim()) {
        command.push_str(" --network ");
        command.push_str(&shell_quote(request.network.trim()));
    }
    let name = request.name.trim();
    if is_safe_name(name) {
        command.push_str(" --name ");
        command.push_str(&shell_quote(name));
    }
    for port in request.ports.iter().map(|port| port.trim()) {
        if !port.is_empty() && is_safe_name(port) {
            command.push_str(" -p ");
            command.push_str(&shell_quote(port));
        }
    }
    for entry in request.env.iter().map(|entry| entry.trim()) {
        if !entry.is_empty() {
            command.push_str(" -e ");
            command.push_str(&shell_quote(entry));
        }
    }
    for volume in request.volumes.iter().map(|volume| volume.trim()) {
        if !volume.is_empty() && is_safe_name(volume) {
            command.push_str(" -v ");
            command.push_str(&shell_quote(volume));
        }
    }
    command.push(' ');
    command.push_str(&shell_quote(&request.reference()));
    let extra = request.command.trim();
    if !extra.is_empty() {
        // The override is a command line, not one argument, so it goes in as typed.
        command.push(' ');
        command.push_str(extra);
    }
    command
}

/// The `--restart` value for a policy name, or `None` for "not set".
pub fn restart_policy_flag(policy: &str) -> Option<&'static str> {
    match policy.trim() {
        "" | "no" => None,
        "always" => Some("always"),
        "unless-stopped" => Some("unless-stopped"),
        "on-failure" => Some("on-failure"),
        _ => None,
    }
}

/// `docker info` as JSON, for the engine status.
pub fn engine_command(socket: &str) -> String {
    format!(
        "docker{} info --format '{{{{json .}}}}'",
        socket_flag(socket)
    )
}

/// Run `command` as root, with the password on stdin when one is given.
pub fn privileged(command: &str, password: Option<&str>) -> String {
    let prefix = match password {
        Some(_) => "sudo -S -p ''",
        None => "sudo -n",
    };
    format!("{prefix} sh -c {}", shell_quote(command))
}

/// Whether a failure means "this needed root".
pub fn needs_root(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "permission denied",
        "are you root",
        "must be run as root",
        "a password is required",
        "a terminal is required",
        "not in the sudoers",
        "operation not permitted",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

/// Parse `docker ps --format '{{json .}}'` output.
pub fn parse_containers(text: &str) -> Vec<Container> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .map(|value| Container {
            id: field(&value, "ID"),
            name: field(&value, "Names").trim_start_matches('/').to_owned(),
            image: field(&value, "Image"),
            command: field(&value, "Command"),
            state: field(&value, "State"),
            status: field(&value, "Status"),
            ports: parse_ports(&field(&value, "Ports")),
            mounts: parse_mounts(&field(&value, "Mounts")),
            networks: split_list(&field(&value, "Networks"), ','),
            labels: parse_labels(&field(&value, "Labels")),
        })
        .filter(|container| !container.id.is_empty() && !container.name.is_empty())
        .collect()
}

/// Parse `0.0.0.0:8080->80/tcp, :::8080->80/tcp` into one entry per container port.
pub fn parse_ports(text: &str) -> Vec<Port> {
    let mut ports: Vec<Port> = Vec::new();
    for entry in split_list(text, ',') {
        let Some((left, right)) = entry.split_once("->") else {
            continue;
        };
        let (port, protocol) = right
            .split_once('/')
            .map_or((right, "tcp"), |(port, protocol)| (port, protocol));
        let Ok(container_port) = port.trim().parse::<u16>() else {
            continue;
        };
        let (host_ip, host_port) = match left.rsplit_once(':') {
            Some((host_ip, host_port)) => (host_ip.trim(), host_port.trim().parse::<u16>().ok()),
            None => ("", left.trim().parse::<u16>().ok()),
        };
        let port = Port {
            host_ip: host_ip.to_owned(),
            host_port,
            container_port,
            protocol: protocol.trim().to_owned(),
        };
        // IPv4 and IPv6 bindings are the same publication.
        if !ports.iter().any(|seen| {
            seen.host_port == port.host_port
                && seen.container_port == port.container_port
                && seen.protocol == port.protocol
        }) {
            ports.push(port);
        }
    }
    ports
}

/// Parse `volume:/data, /host:/srv:ro` into mounts.
pub fn parse_mounts(text: &str) -> Vec<Mount> {
    split_list(text, ',')
        .into_iter()
        .filter_map(|entry| {
            let mut parts = entry.split(':');
            let source = parts.next()?.trim();
            let destination = parts.next()?.trim();
            let mode = parts.next().unwrap_or_default().trim();
            if source.is_empty() || destination.is_empty() {
                return None;
            }
            let kind = if source.starts_with('/') {
                "bind"
            } else {
                "volume"
            };
            Some(Mount {
                kind: kind.to_owned(),
                source: source.to_owned(),
                destination: destination.to_owned(),
                writable: !mode.split(',').any(|flag| flag == "ro"),
            })
        })
        .collect()
}

/// Parse `a=1,b=two` into label pairs.
pub fn parse_labels(text: &str) -> Vec<(String, String)> {
    split_list(text, ',')
        .into_iter()
        .filter_map(|entry| {
            let (key, value) = entry.split_once('=')?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some((key.to_owned(), value.trim().to_owned()))
        })
        .collect()
}

/// Split on `separator`, ignoring empty items and trimming each.
fn split_list(text: &str, separator: char) -> Vec<String> {
    text.split(separator)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}

/// A JSON field as text, with docker's `<no value>`/`null` treated as empty.
fn field(value: &Value, key: &str) -> String {
    match value.get(key) {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) | None => String::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| item.as_str().unwrap_or_default())
            .collect::<Vec<_>>()
            .join(" "),
        Some(other) => other.to_string(),
    }
}

/// Parse `docker stats --no-stream --format '{{json .}}'` output.
pub fn parse_stats(text: &str) -> Option<Stats> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let memory = field(&value, "MemUsage");
    let network = field(&value, "NetIO");
    let block = field(&value, "BlockIO");
    let (used, limit) = split_pair(&memory);
    let (net_rx, net_tx) = split_pair(&network);
    let (block_read, block_write) = split_pair(&block);
    Some(Stats {
        cpu_percent: percent(&field(&value, "CPUPerc")).unwrap_or_default(),
        memory_used: parse_size(used).unwrap_or_default(),
        memory_limit: parse_size(limit).unwrap_or_default(),
        memory_percent: percent(&field(&value, "MemPerc")).unwrap_or_default(),
        pids: field(&value, "PIDs").trim().parse().unwrap_or_default(),
        net_rx: parse_size(net_rx).unwrap_or_default(),
        net_tx: parse_size(net_tx).unwrap_or_default(),
        block_read: parse_size(block_read).unwrap_or_default(),
        block_write: parse_size(block_write).unwrap_or_default(),
    })
}

/// `1.2MiB / 2GiB` into its two halves.
fn split_pair(text: &str) -> (&str, &str) {
    match text.split_once('/') {
        Some((left, right)) => (left.trim(), right.trim()),
        None => (text.trim(), ""),
    }
}

/// `0.05%` into `0.05`.
pub fn percent(text: &str) -> Option<f64> {
    text.trim().trim_end_matches('%').trim().parse::<f64>().ok()
}

/// `1.5MiB` into bytes. Decimal units count 1000, binary units 1024, as docker uses them.
pub fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let digits: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if digits.is_empty() {
        return None;
    }
    let number: f64 = digits.parse().ok()?;
    let unit = text[digits.len()..].trim();
    let factor = match unit {
        "" | "B" => 1.0,
        "kB" | "KB" => 1_000.0,
        "MB" => 1_000_000.0,
        "GB" => 1_000_000_000.0,
        "TB" => 1_000_000_000_000.0,
        "KiB" => 1_024.0,
        "MiB" => 1_048_576.0,
        "GiB" => 1_073_741_824.0,
        "TiB" => 1_099_511_627_776.0,
        _ => return None,
    };
    Some((number * factor) as u64)
}

/// Parse `docker images --format '{{json .}}'` output, one line per reference.
///
/// Docker prints a line per tag, so references are grouped by image id.
pub fn parse_images(text: &str, containers: &[Container]) -> Vec<Image> {
    let mut images: Vec<Image> = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line.trim()) else {
            continue;
        };
        let id = field(&value, "ID");
        if id.is_empty() {
            continue;
        }
        let repository = field(&value, "Repository");
        let tag = field(&value, "Tag");
        let reference = if repository == "<none>" || tag == "<none>" {
            "<none>".to_owned()
        } else {
            format!("{repository}:{tag}")
        };
        let size = parse_size(&field(&value, "Size")).unwrap_or_default();
        let created = field(&value, "CreatedAt");
        match images.iter_mut().find(|image| image.id == id) {
            Some(image) => {
                if !image.references.contains(&reference) {
                    image.references.push(reference);
                }
            }
            None => images.push(Image {
                id,
                references: vec![reference],
                created,
                size,
                containers: 0,
            }),
        }
    }
    for image in &mut images {
        image.containers = containers
            .iter()
            .filter(|container| {
                image
                    .references
                    .iter()
                    .any(|reference| reference == &container.image)
            })
            .count();
    }
    images
}

/// Parse `docker volume ls --format '{{json .}}'` output.
pub fn parse_volumes(text: &str) -> Vec<Volume> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .map(|value| Volume {
            name: field(&value, "Name"),
            driver: field(&value, "Driver"),
            scope: field(&value, "Scope"),
            mountpoint: field(&value, "Mountpoint"),
        })
        .filter(|volume| !volume.name.is_empty())
        .collect()
}

/// Parse `docker network ls --format '{{json .}}'` output.
pub fn parse_networks(text: &str) -> Vec<Network> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .map(|value| Network {
            id: field(&value, "ID"),
            name: field(&value, "Name"),
            driver: field(&value, "Driver"),
            scope: field(&value, "Scope"),
            internal: value
                .get("Internal")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .filter(|network| !network.name.is_empty())
        .collect()
}

/// Parse `docker inspect` output for one container.
pub fn parse_detail(text: &str) -> Option<Detail> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    let document = value.as_array()?.first()?;
    let state = document.get("State").cloned().unwrap_or(Value::Null);
    let config = document.get("Config").cloned().unwrap_or(Value::Null);
    let host = document.get("HostConfig").cloned().unwrap_or(Value::Null);

    let mut mounts = Vec::new();
    if let Some(list) = document.get("Mounts").and_then(Value::as_array) {
        for mount in list {
            let name = field(mount, "Name");
            let source = if name.is_empty() {
                field(mount, "Source")
            } else {
                name
            };
            let writable = mount.get("RW").and_then(Value::as_bool).unwrap_or(true);
            mounts.push(format!(
                "{source} → {} ({})",
                field(mount, "Destination"),
                if writable { "rw" } else { "ro" }
            ));
        }
    }
    let mut networks = Vec::new();
    if let Some(map) = document
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    {
        for (name, settings) in map {
            let address = field(settings, "IPAddress");
            networks.push(if address.is_empty() {
                name.clone()
            } else {
                format!("{name} ({address})")
            });
        }
    }
    let mut ports = Vec::new();
    if let Some(map) = host.get("PortBindings").and_then(Value::as_object) {
        for (container, bindings) in map {
            let Some(bindings) = bindings.as_array() else {
                continue;
            };
            for binding in bindings {
                let host_port = field(binding, "HostPort");
                let ip = field(binding, "HostIp");
                ports.push(format!(
                    "{}:{container}",
                    if ip.is_empty() || ip == "0.0.0.0" {
                        host_port
                    } else {
                        format!("{ip}:{host_port}")
                    }
                ));
            }
        }
    }
    let mut command = field(&config, "Entrypoint");
    let cmd = field(&config, "Cmd");
    if !cmd.is_empty() {
        if !command.is_empty() {
            command.push(' ');
        }
        command.push_str(&cmd);
    }

    Some(Detail {
        id: field(document, "Id"),
        name: field(document, "Name").trim_start_matches('/').to_owned(),
        image: field(&config, "Image"),
        created: field(document, "Created"),
        status: field(&state, "Status"),
        started_at: field(&state, "StartedAt"),
        finished_at: field(&state, "FinishedAt"),
        exit_code: state.get("ExitCode").and_then(Value::as_i64),
        pid: state
            .get("Pid")
            .and_then(Value::as_i64)
            .filter(|pid| *pid > 0),
        restart_count: document
            .get("RestartCount")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        platform: field(document, "Platform"),
        command,
        workdir: field(&config, "WorkingDir"),
        user: field(&config, "User"),
        tty: config.get("Tty").and_then(Value::as_bool).unwrap_or(false),
        restart_policy: field(
            &host.get("RestartPolicy").cloned().unwrap_or(Value::Null),
            "Name",
        ),
        network_mode: field(&host, "NetworkMode"),
        privileged: host
            .get("Privileged")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        auto_remove: host
            .get("AutoRemove")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        env: string_list(config.get("Env")),
        ports,
        networks,
        mounts,
        raw: serde_json::to_string_pretty(&value).unwrap_or_default(),
    })
}

/// `1536` as `1.5 KiB`, for sizes docker reports in bytes.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// The script that replaces a container with a fresh one built from the same
/// configuration, as the reference's `updateService` did: pull the tag, stop and
/// rename the old container, create and start the replacement, and put the old
/// one back when the replacement refuses to start.
pub fn recreate_script(socket: &str, document: &Value) -> Option<String> {
    let create = recreate_command(socket, document)?;
    let name = field(document, "Name").trim_start_matches('/').to_owned();
    let id = field(document, "Id");
    let image = field(document.get("Config")?, "Image");
    if name.is_empty() || id.is_empty() {
        return None;
    }
    let docker = format!("docker{}", socket_flag(socket));
    let old = shell_quote(&format!("{name}-wassh-old"));
    let name = shell_quote(&name);
    let id = shell_quote(&id);
    let image = shell_quote(&image);
    Some(format!(
        "set -e; {docker} pull {image} >/dev/null; \
         {docker} stop {id} >/dev/null 2>&1 || true; \
         {docker} rename {id} {old} >/dev/null 2>&1 || true; \
         if {create} && {docker} start {name} >/dev/null; then \
         {docker} rm -f {old} >/dev/null 2>&1 || true; \
         else {docker} rm -f {name} >/dev/null 2>&1 || true; \
         {docker} rename {old} {name} >/dev/null 2>&1 || true; \
         {docker} start {name} >/dev/null 2>&1 || true; exit 1; fi"
    ))
}

/// A JSON array of strings, ignoring anything else.
fn string_list(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

/// The `docker run` command that recreates `document` (a container's inspect
/// output) with the same configuration.
///
/// Port of the reference's `recreate.ts`: ports, mounts, environment, restart
/// policy, networks, labels and the entrypoint/command all carry over.
pub fn recreate_command(socket: &str, document: &Value) -> Option<String> {
    let config = document.get("Config")?;
    let host = document.get("HostConfig");
    let image = field(config, "Image");
    if image.is_empty() {
        return None;
    }
    let mut command = format!("docker{} create", socket_flag(socket));
    command.push_str(" --name ");
    command.push_str(&shell_quote(
        field(document, "Name").trim_start_matches('/'),
    ));
    let policy = host
        .and_then(|host| host.get("RestartPolicy"))
        .map(|policy| field(policy, "Name"))
        .unwrap_or_default();
    if let Some(policy) = restart_policy_flag(&policy) {
        command.push_str(" --restart ");
        command.push_str(policy);
    }
    if host
        .and_then(|host| host.get("AutoRemove"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        command.push_str(" --rm");
    }
    if host
        .and_then(|host| host.get("Privileged"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        command.push_str(" --privileged");
    }
    let user = field(config, "User");
    if !user.is_empty() {
        command.push_str(" -u ");
        command.push_str(&shell_quote(&user));
    }
    let workdir = field(config, "WorkingDir");
    if !workdir.is_empty() {
        command.push_str(" -w ");
        command.push_str(&shell_quote(&workdir));
    }
    if config.get("Tty").and_then(Value::as_bool).unwrap_or(false) {
        command.push_str(" -t");
    }
    if let Some(bindings) = host
        .and_then(|host| host.get("PortBindings"))
        .and_then(Value::as_object)
    {
        for (container, entries) in bindings {
            let Some(entries) = entries.as_array() else {
                continue;
            };
            if entries.is_empty() {
                command.push_str(" --publish-all=false -p ");
                command.push_str(&shell_quote(container));
                continue;
            }
            for entry in entries {
                let host_port = field(entry, "HostPort");
                let ip = field(entry, "HostIp");
                let mapping = if ip.is_empty() || ip == "0.0.0.0" {
                    format!("{host_port}:{container}")
                } else {
                    format!("{ip}:{host_port}:{container}")
                };
                command.push_str(" -p ");
                command.push_str(&shell_quote(&mapping));
            }
        }
    }
    for mount in document
        .get("Mounts")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let name = field(mount, "Name");
        let source = if name.is_empty() {
            field(mount, "Source")
        } else {
            name
        };
        let writable = mount.get("RW").and_then(Value::as_bool).unwrap_or(true);
        command.push_str(" -v ");
        command.push_str(&shell_quote(&format!(
            "{source}:{}{}",
            field(mount, "Destination"),
            if writable { "" } else { ":ro" }
        )));
    }
    for entry in string_list(config.get("Env")) {
        command.push_str(" -e ");
        command.push_str(&shell_quote(&entry));
    }
    if let Some(labels) = config.get("Labels").and_then(Value::as_object) {
        for (key, value) in labels {
            command.push_str(" --label ");
            command.push_str(&shell_quote(&format!(
                "{key}={}",
                value.as_str().unwrap_or_default()
            )));
        }
    }
    if let Some(map) = document
        .pointer("/NetworkSettings/Networks")
        .and_then(Value::as_object)
    {
        for name in map.keys() {
            if name == "bridge" || name == "host" || name == "none" {
                continue;
            }
            command.push_str(" --network ");
            command.push_str(&shell_quote(name));
            break;
        }
    }
    let entrypoint = field(config, "Entrypoint");
    if !entrypoint.is_empty() {
        command.push_str(" --entrypoint ");
        command.push_str(&shell_quote(&entrypoint));
    }
    command.push(' ');
    command.push_str(&shell_quote(&image));
    let cmd = field(config, "Cmd");
    if !cmd.is_empty() {
        command.push(' ');
        command.push_str(&cmd);
    }
    Some(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PS: &str = concat!(
        r#"{"ID":"8f2c1b7d9e01aaaa","Names":"web","Image":"nginx:latest","#,
        r#""Command":"\"nginx -g 'daemon off;'\"","State":"running","#,
        r#""Status":"Up 3 hours","Ports":"0.0.0.0:8080->80/tcp, :::8080->80/tcp","#,
        r#""Labels":"com.example.tier=web","Mounts":"site:/usr/share/nginx/html","#,
        r#""Networks":"bridge"}"#,
        "\n",
        r#"{"ID":"1a2b3c4d5e6f","Names":"db","Image":"postgres:16","Command":"postgres","#,
        r#""State":"exited","Status":"Exited (1) 2 days ago","Ports":"","Labels":"","#,
        r#""Mounts":"/srv/db:/var/lib/postgresql/data:ro","Networks":"bridge,backend"}"#,
    );

    #[test]
    fn the_container_list_parses_every_field_the_rows_show() {
        let containers = parse_containers(PS);
        assert_eq!(containers.len(), 2);
        let web = &containers[0];
        assert_eq!(web.name, "web");
        assert_eq!(web.image, "nginx:latest");
        assert!(web.is_running());
        assert_eq!(
            web.ports,
            vec![Port {
                host_ip: "0.0.0.0".to_owned(),
                host_port: Some(8080),
                container_port: 80,
                protocol: "tcp".to_owned()
            }]
        );
        assert_eq!(web.ports[0].label(), "8080→80/tcp");
        assert_eq!(web.mounts[0].kind, "volume");
        assert!(web.mounts[0].writable);
        assert_eq!(
            web.labels,
            vec![("com.example.tier".to_owned(), "web".to_owned())]
        );
        assert_eq!(web.short_id(), "8f2c1b7d9e01");

        let db = &containers[1];
        assert!(!db.is_running());
        assert_eq!(db.networks, ["bridge", "backend"]);
        assert_eq!(db.mounts[0].kind, "bind");
        assert!(!db.mounts[0].writable);
        assert!(db.ports.is_empty());
    }

    #[test]
    fn only_stopped_containers_get_a_start_button() {
        let containers = parse_containers(PS);
        assert_eq!(
            containers[0].actions(),
            vec![
                Action::Stop,
                Action::Restart,
                Action::Pause,
                Action::Kill,
                Action::Remove
            ]
        );
        assert_eq!(containers[1].actions(), vec![Action::Start, Action::Remove]);
        assert_eq!(containers[0].rank(), 0);
        assert_eq!(containers[1].rank(), 3);
    }

    #[test]
    fn a_paused_container_offers_unpause() {
        let container = Container {
            state: "paused".to_owned(),
            ..Container::default()
        };
        assert!(container.is_paused());
        assert!(container.actions().contains(&Action::Unpause));
    }

    #[test]
    fn commands_are_built_from_safe_ids_only() {
        assert_eq!(
            action_command(DEFAULT_SOCKET, "abc123", Action::Stop).unwrap(),
            "docker stop -t 10 'abc123'"
        );
        assert!(action_command(DEFAULT_SOCKET, "abc; rm -rf /", Action::Stop).is_none());
        assert_eq!(
            logs_command(DEFAULT_SOCKET, "abc123", 50).unwrap(),
            "docker logs --tail 50 'abc123' 2>&1"
        );
        assert_eq!(
            ps_command(DEFAULT_SOCKET, true),
            "docker ps -a --no-trunc --format '{{json .}}'"
        );
        // A non-default socket is passed to the CLI as a unix host.
        assert_eq!(
            ps_command("/run/user/1000/docker.sock", false),
            "docker -H 'unix:///run/user/1000/docker.sock' ps --no-trunc --format '{{json .}}'"
        );
    }

    #[test]
    fn a_run_request_becomes_one_command() {
        let request = RunRequest {
            image: "nginx".to_owned(),
            tag: "alpine".to_owned(),
            name: "web".to_owned(),
            command: "nginx -g 'daemon off;'".to_owned(),
            ports: vec!["8080:80".to_owned()],
            env: vec!["TZ=UTC".to_owned()],
            volumes: vec!["site:/data:ro".to_owned()],
            restart_policy: "unless-stopped".to_owned(),
            network: "backend".to_owned(),
            detach: true,
            auto_remove: false,
        };
        assert_eq!(
            run_command(DEFAULT_SOCKET, &request),
            "docker run -d --restart unless-stopped --network 'backend' --name 'web' \
             -p '8080:80' -e 'TZ=UTC' -v 'site:/data:ro' 'nginx:alpine' nginx -g 'daemon off;'"
        );
        assert_eq!(request.reference(), "nginx:alpine");
    }

    #[test]
    fn stats_are_read_from_the_cli_line() {
        let line = r#"{"Container":"abc","Name":"web","CPUPerc":"0.05%","MemUsage":"1.5MiB / 7.7GiB",
            "MemPerc":"0.02%","NetIO":"1.2kB / 3.4kB","BlockIO":"0B / 4MiB","PIDs":"7"}"#;
        let stats = parse_stats(line).unwrap();
        assert_eq!(stats.cpu_percent, 0.05);
        assert_eq!(stats.memory_used, 1_572_864);
        assert_eq!(stats.memory_limit, 8_267_812_044);
        assert_eq!(stats.net_rx, 1_200);
        assert_eq!(stats.net_tx, 3_400);
        assert_eq!(stats.block_read, 0);
        assert_eq!(stats.block_write, 4_194_304);
        assert_eq!(stats.pids, 7);
    }

    #[test]
    fn sizes_and_percentages_parse() {
        assert_eq!(parse_size("0B"), Some(0));
        assert_eq!(parse_size("512"), Some(512));
        assert_eq!(parse_size("2GiB"), Some(2_147_483_648));
        assert_eq!(parse_size("7.7GiB"), Some(8_267_812_044));
        assert_eq!(parse_size("nonsense"), None);
        assert_eq!(percent("12.5%"), Some(12.5));
        assert_eq!(percent("--"), None);
    }

    #[test]
    fn images_are_grouped_by_id() {
        let text = concat!(
            r#"{"ID":"sha256:aaa","Repository":"nginx","Tag":"latest","CreatedAt":"2 weeks ago","Size":"187MB"}"#,
            "\n",
            r#"{"ID":"sha256:aaa","Repository":"nginx","Tag":"1.27","CreatedAt":"2 weeks ago","Size":"187MB"}"#,
            "\n",
            r#"{"ID":"sha256:bbb","Repository":"<none>","Tag":"<none>","CreatedAt":"3 weeks ago","Size":"10MB"}"#,
        );
        let containers = parse_containers(PS);
        let images = parse_images(text, &containers);
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].references, ["nginx:latest", "nginx:1.27"]);
        assert_eq!(images[0].size, 187_000_000);
        assert_eq!(
            images[0].containers, 1,
            "nginx:latest runs the web container"
        );
        assert_eq!(images[1].references, ["<none>"]);
        assert_eq!(images[1].containers, 0);
    }

    #[test]
    fn volumes_and_networks_parse() {
        let volumes = parse_volumes(
            r#"{"Name":"site","Driver":"local","Scope":"local","Mountpoint":"/var/lib/docker/volumes/site/_data"}"#,
        );
        assert_eq!(volumes[0].name, "site");
        assert_eq!(volumes[0].mountpoint, "/var/lib/docker/volumes/site/_data");

        let networks = parse_networks(
            r#"{"ID":"9f1c2d3e4a5b","Name":"backend","Driver":"bridge","Scope":"local","Internal":true}"#,
        );
        assert_eq!(networks[0].name, "backend");
        assert!(networks[0].internal);
        assert_eq!(networks[0].short_id(), "9f1c2d3e4a5b");
    }

    #[test]
    fn the_detail_panel_reads_the_inspect_document() {
        let text = r#"[{
            "Id": "8f2c1b7d9e01aaaa",
            "Name": "/web",
            "Created": "2026-10-01T10:00:00Z",
            "Platform": "linux",
            "RestartCount": 2,
            "State": {"Status":"running","StartedAt":"2026-10-01T10:00:01Z","FinishedAt":"0001-01-01T00:00:00Z",
                      "ExitCode":0,"Pid":4211},
            "Config": {"Image":"nginx:latest","Env":["TZ=UTC","HOME=/root"],"WorkingDir":"/srv","User":"www",
                       "Tty":true,"Entrypoint":["/docker-entrypoint.sh"],"Cmd":["nginx","-g","daemon off;"],
                       "Labels":{"tier":"web"}},
            "HostConfig": {"RestartPolicy":{"Name":"unless-stopped"},"NetworkMode":"bridge",
                           "Privileged":false,"AutoRemove":true,
                           "PortBindings":{"80/tcp":[{"HostIp":"","HostPort":"8080"}]}},
            "Mounts": [{"Type":"volume","Name":"site","Source":"/var/lib/docker/volumes/site/_data",
                        "Destination":"/usr/share/nginx/html","RW":true}],
            "NetworkSettings": {"Networks":{"bridge":{"IPAddress":"172.17.0.2"}}}
        }]"#;
        let detail = parse_detail(text).unwrap();
        assert_eq!(detail.name, "web");
        assert_eq!(detail.image, "nginx:latest");
        assert_eq!(detail.pid, Some(4211));
        assert_eq!(detail.restart_count, 2);
        assert_eq!(detail.command, "/docker-entrypoint.sh nginx -g daemon off;");
        assert!(detail.tty);
        assert!(detail.auto_remove);
        assert_eq!(detail.restart_policy, "unless-stopped");
        assert_eq!(detail.env, ["TZ=UTC", "HOME=/root"]);
        assert_eq!(detail.ports, ["8080:80/tcp"]);
        assert_eq!(detail.networks, ["bridge (172.17.0.2)"]);
        assert_eq!(detail.mounts, ["site → /usr/share/nginx/html (rw)"]);
        assert!(detail.raw.contains("\"Id\""));
    }

    #[test]
    fn recreating_keeps_the_configuration() {
        let document: Value = serde_json::from_str(
            r#"{
                "Name": "/web",
                "Config": {"Image":"nginx:latest","Env":["TZ=UTC"],"WorkingDir":"/srv","User":"www",
                           "Tty":true,"Entrypoint":["/docker-entrypoint.sh"],"Cmd":["nginx","-g","daemon off;"],
                           "Labels":{"tier":"web"}},
                "HostConfig": {"RestartPolicy":{"Name":"unless-stopped"},"AutoRemove":false,
                               "Privileged":false,"PortBindings":{"80/tcp":[{"HostIp":"","HostPort":"8080"}]}},
                "Mounts": [{"Type":"volume","Name":"site","Destination":"/srv","RW":false}],
                "NetworkSettings": {"Networks":{"backend":{"IPAddress":"172.18.0.2"}}}
            }"#,
        )
        .unwrap();
        let command = recreate_command(DEFAULT_SOCKET, &document).unwrap();
        assert_eq!(
            command,
            "docker create --name 'web' --restart unless-stopped -u 'www' -w '/srv' -t \
             -p '8080:80/tcp' -v 'site:/srv:ro' -e 'TZ=UTC' --label 'tier=web' --network 'backend' \
             --entrypoint '/docker-entrypoint.sh' 'nginx:latest' nginx -g daemon off;"
        );
    }

    #[test]
    fn the_update_script_pulls_recreates_and_restores_on_failure() {
        let document: Value = serde_json::from_str(
            r#"{"Id":"8f2c1b7d9e01","Name":"/web",
                "Config":{"Image":"nginx:latest","Env":[],"Entrypoint":null,"Cmd":["nginx"]},
                "HostConfig":{"RestartPolicy":{"Name":"always"}},
                "Mounts":[],"NetworkSettings":{"Networks":{"bridge":{}}}}"#,
        )
        .unwrap();
        let script = recreate_script(DEFAULT_SOCKET, &document).unwrap();
        assert!(
            script.starts_with("set -e; docker pull 'nginx:latest'"),
            "{script}"
        );
        assert!(script.contains("docker stop '8f2c1b7d9e01'"), "{script}");
        assert!(
            script.contains("docker rename '8f2c1b7d9e01' 'web-wassh-old'"),
            "{script}"
        );
        assert!(script.contains("docker create --name 'web'"), "{script}");
        assert!(script.contains("docker start 'web'"), "{script}");
        // A replacement that will not start puts the old container back.
        assert!(
            script.contains("docker rename 'web-wassh-old' 'web'"),
            "{script}"
        );
    }

    #[test]
    fn sizes_read_in_binary_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(187_000_000), "178.3 MiB");
    }

    #[test]
    fn privileged_commands_feed_the_password_on_stdin() {
        assert_eq!(privileged("docker ps", None), "sudo -n sh -c 'docker ps'");
        assert_eq!(
            privileged("apt-get install -y docker.io", Some("hunter2")),
            "sudo -S -p '' sh -c 'apt-get install -y docker.io'"
        );
    }

    #[test]
    fn privilege_failures_are_recognised() {
        assert!(needs_root(
            "permission denied while trying to connect to the Docker daemon socket"
        ));
        assert!(needs_root("sudo: a password is required"));
        assert!(needs_root("Error: must be run as root"));
        assert!(!needs_root("No such container: web"));
    }
}
