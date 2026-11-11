//! The Docker engine: is there a daemon, which version, and how would this host
//! install or upgrade it.
//!
//! Port of the reference's `engineInstall.ts` and the engine half of
//! `main.ts`. The privileged commands are built here and only run once the user
//! has given a password (or `sudo -n` works), so nothing privileged happens
//! while the panel is merely open.

use serde_json::Value;

/// Probe that tells us which package manager owns Docker.
pub const DETECT_COMMAND: &str = concat!(
    ". /etc/os-release 2>/dev/null || true; ",
    "echo \"OS_ID=${ID:-unknown}\"; ",
    "echo \"OS_LIKE=${ID_LIKE:-}\"; ",
    "for m in apt-get dnf yum zypper pacman apk; do command -v \"$m\" >/dev/null 2>&1 && echo \"HAS_$m=1\"; done; ",
    // The last lookup fails on a host missing that tool, which would report a failed probe
    // for an answer that arrived intact.
    "true"
);

/// What the host installs packages with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Manager {
    Apt,
    Dnf,
    Yum,
    Zypper,
    Pacman,
    Apk,
    Unknown,
}

impl Manager {
    /// Wording shown next to the plan.
    pub fn label(self) -> &'static str {
        match self {
            Self::Apt => "apt-get (Debian/Ubuntu)",
            Self::Dnf => "dnf (Fedora/RHEL)",
            Self::Yum => "yum (RHEL/CentOS)",
            Self::Zypper => "zypper (openSUSE/SLES)",
            Self::Pacman => "pacman (Arch)",
            Self::Apk => "apk (Alpine)",
            Self::Unknown => "unknown package manager",
        }
    }

    /// The privileged command that installs the engine, without `sudo`.
    pub fn install_command(self) -> Option<&'static str> {
        Some(match self {
            Self::Apt => concat!(
                "export DEBIAN_FRONTEND=noninteractive && apt-get update && ",
                "apt-get install -y docker.io && ",
                "systemctl enable --now docker 2>/dev/null || true"
            ),
            Self::Dnf => concat!(
                "dnf install -y moby-engine docker 2>/dev/null || dnf install -y moby-engine; ",
                "systemctl enable --now docker 2>/dev/null || true"
            ),
            Self::Yum => concat!(
                "yum install -y docker && ",
                "systemctl enable --now docker 2>/dev/null || true"
            ),
            Self::Zypper => concat!(
                "zypper --non-interactive install docker && ",
                "systemctl enable --now docker 2>/dev/null || true"
            ),
            Self::Pacman => concat!(
                "pacman -Sy --noconfirm docker && ",
                "systemctl enable --now docker 2>/dev/null || true"
            ),
            Self::Apk => concat!(
                "apk add --no-cache docker && ",
                "rc-update add docker default 2>/dev/null || true && ",
                "service docker start 2>/dev/null || true"
            ),
            Self::Unknown => return None,
        })
    }

    /// The privileged command that upgrades the engine, without `sudo`.
    pub fn upgrade_command(self) -> Option<&'static str> {
        Some(match self {
            Self::Apt => concat!(
                "export DEBIAN_FRONTEND=noninteractive && apt-get update && ",
                "apt-get install -y --only-upgrade docker.io containerd runc || ",
                "apt-get install -y --only-upgrade docker.io; ",
                "systemctl try-restart docker 2>/dev/null || true"
            ),
            Self::Dnf => concat!(
                "dnf upgrade -y moby-engine docker 2>/dev/null || dnf upgrade -y moby-engine; ",
                "systemctl try-restart docker 2>/dev/null || true"
            ),
            Self::Yum => concat!(
                "yum update -y docker && ",
                "systemctl try-restart docker 2>/dev/null || true"
            ),
            Self::Zypper => concat!(
                "zypper --non-interactive update docker && ",
                "systemctl try-restart docker 2>/dev/null || true"
            ),
            Self::Pacman => concat!(
                "pacman -Syu --noconfirm docker && ",
                "systemctl try-restart docker 2>/dev/null || true"
            ),
            Self::Apk => concat!(
                "apk upgrade docker && ",
                "rc-service docker restart 2>/dev/null || ",
                "service docker restart 2>/dev/null || true"
            ),
            Self::Unknown => return None,
        })
    }

    /// The privileged command that starts the daemon an installed engine ships with.
    pub fn start_command(self) -> Option<&'static str> {
        Some(match self {
            Self::Apt | Self::Dnf | Self::Yum | Self::Zypper | Self::Pacman => {
                "systemctl start docker"
            }
            Self::Apk => "rc-service docker start 2>/dev/null || service docker start",
            Self::Unknown => return None,
        })
    }
}

/// The winget package that installs Docker Desktop on Windows.
pub const DOCKER_DESKTOP_PACKAGE: &str = "Docker.DockerDesktop";

/// Windows' script that starts the Docker Desktop install elevated, without waiting.
///
/// `winget` needs an administrator, so the UAC prompt belongs to the user; the script
/// returns as soon as that is answered and the installer keeps running on its own, which
/// keeps the pane's command from being killed at the local command timeout.
pub fn windows_install_command() -> String {
    format!(
        "Start-Process -FilePath winget -ArgumentList \
         'install','-e','--id','{DOCKER_DESKTOP_PACKAGE}',\
         '--accept-package-agreements','--accept-source-agreements' -Verb RunAs"
    )
}

/// What the pane says when the CLI is there but the daemon is not.
pub const DAEMON_DOWN_WINDOWS: &str =
    "Docker Desktop is installed but its engine is not running. Start Docker Desktop.";

/// What the pane says when `docker info` answered, but not with an engine's own words.
pub const UNEXPECTED_ANSWER: &str =
    "The docker command answered, but not with a Docker engine version.";

/// Which manager the distro id, its family, and the installed tools point at.
pub fn manager_from_probe(output: &str) -> Manager {
    let value_of = |key: &str| -> String {
        output
            .lines()
            .find_map(|line| line.trim().strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase()
    };
    let installed = |name: &str| output.contains(&format!("HAS_{name}=1"));
    let family = format!("{} {}", value_of("OS_ID"), value_of("OS_LIKE"));

    if [
        "ubuntu",
        "debian",
        "mint",
        "pop",
        "raspbian",
        "kali",
        "devuan",
        "elementary",
        "zorin",
        "deepin",
    ]
    .iter()
    .any(|name| family.contains(name))
    {
        return Manager::Apt;
    }
    if family.contains("fedora") {
        return Manager::Dnf;
    }
    if [
        "rhel",
        "centos",
        "rocky",
        "almalinux",
        "oracle",
        "amazon",
        "scientific",
    ]
    .iter()
    .any(|name| family.contains(name))
    {
        return if installed("dnf") {
            Manager::Dnf
        } else {
            Manager::Yum
        };
    }
    if ["opensuse", "suse", "sles"]
        .iter()
        .any(|name| family.contains(name))
    {
        return Manager::Zypper;
    }
    if ["arch", "manjaro", "endeavour"]
        .iter()
        .any(|name| family.contains(name))
    {
        return Manager::Pacman;
    }
    if family.contains("alpine") {
        return Manager::Apk;
    }
    // No recognisable distribution: fall back to whichever manager is installed.
    for (tool, manager) in [
        ("apt-get", Manager::Apt),
        ("dnf", Manager::Dnf),
        ("yum", Manager::Yum),
        ("zypper", Manager::Zypper),
        ("pacman", Manager::Pacman),
    ] {
        if installed(tool) {
            return manager;
        }
    }
    if installed("apk") {
        return Manager::Apk;
    }
    Manager::Unknown
}

/// How the daemon looks right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not asked yet.
    Checking,
    /// Answering.
    Ready,
    /// The `docker` command is not on this host at all.
    Missing,
    /// The command is there but no daemon answers.
    Unavailable,
    /// The daemon is there but this user may not talk to it.
    Forbidden,
    /// Some other failure.
    Error,
}

/// The engine as the panel shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// What state the probe found.
    pub state: Option<State>,
    /// Server version, e.g. `27.1.1`.
    pub version: String,
    /// Operating system the daemon reports.
    pub os: String,
    /// Architecture the daemon reports.
    pub arch: String,
    /// Containers running.
    pub running: u64,
    /// Containers paused.
    pub paused: u64,
    /// Containers stopped.
    pub stopped: u64,
    /// Images.
    pub images: u64,
    /// Why it is not usable, when it is not.
    pub message: String,
}

impl Status {
    /// Whether the engine answers.
    pub fn is_ready(&self) -> bool {
        self.state == Some(State::Ready)
    }

    /// The one-line summary the header shows.
    pub fn summary(&self) -> String {
        match self.state {
            None | Some(State::Checking) => "Checking the engine…".to_owned(),
            Some(State::Ready) => format!(
                "Engine {} · {} running, {} stopped · {} image{}",
                self.version,
                self.running,
                self.stopped,
                self.images,
                if self.images == 1 { "" } else { "s" }
            ),
            Some(State::Forbidden) => "Permission denied for the Docker daemon".to_owned(),
            Some(State::Missing) => "Docker is not installed on this host".to_owned(),
            Some(State::Unavailable) => {
                if self.message.is_empty() {
                    "No Docker daemon".to_owned()
                } else {
                    self.message.clone()
                }
            }
            Some(State::Error) => {
                if self.message.is_empty() {
                    "The engine check failed".to_owned()
                } else {
                    self.message.clone()
                }
            }
        }
    }
}

/// Read `docker info --format '{{json .}}'`.
///
/// A daemon that answers always names its own version, so an object without one came from
/// something else wearing the `docker` name — not from an engine the pane can drive.
pub fn parse_info(text: &str) -> Option<Status> {
    let value: Value = serde_json::from_str(text.trim()).ok()?;
    if !value.is_object() {
        return None;
    }
    let version = string(&value, "ServerVersion");
    if version.is_empty() {
        return None;
    }
    let number = |key: &str| value.get(key).and_then(Value::as_u64).unwrap_or_default();
    Some(Status {
        state: Some(State::Ready),
        version,
        os: string(&value, "OperatingSystem"),
        arch: string(&value, "Architecture"),
        running: number("ContainersRunning"),
        paused: number("ContainersPaused"),
        stopped: number("ContainersStopped"),
        images: number("Images"),
        message: String::new(),
    })
}

/// Explain why the probe failed.
pub fn status_from_failure(message: &str) -> Status {
    let lower = message.to_ascii_lowercase();
    let forbidden = lower.contains("permission denied")
        || lower.contains("got permission denied")
        || lower.contains("are you root");
    // Both shells say it in their own words: `sh: 1: docker: not found` on Unix,
    // `'docker' is not recognized …` in `cmd.exe` and PowerShell.
    let missing = lower.contains("docker: not found")
        || lower.contains("docker: command not found")
        || lower.contains("is not recognized as an internal or external command")
        || lower.contains("is not recognized as the name of a cmdlet");
    let unavailable = lower.contains("cannot connect to the docker daemon")
        || lower.contains("is the docker daemon running")
        || lower.contains("no such file or directory");
    Status {
        state: Some(if forbidden {
            State::Forbidden
        } else if missing {
            State::Missing
        } else if unavailable {
            State::Unavailable
        } else {
            State::Error
        }),
        message: message.trim().to_owned(),
        ..Status::default()
    }
}

fn string(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_probe_maps_families_and_tools_to_a_manager() {
        assert_eq!(
            manager_from_probe("OS_ID=ubuntu\nOS_LIKE=debian\nHAS_apt-get=1"),
            Manager::Apt
        );
        assert_eq!(
            manager_from_probe("OS_ID=fedora\nOS_LIKE=\nHAS=1"),
            Manager::Dnf
        );
        // The family decides; `HAS_dnf` only breaks ties inside it.
        assert_eq!(
            manager_from_probe("OS_ID=rocky\nOS_LIKE=rhel\nHAS_yum=1"),
            Manager::Yum
        );
        assert_eq!(
            manager_from_probe("OS_ID=rocky\nOS_LIKE=rhel\nHAS_dnf=1\nHAS_yum=1"),
            Manager::Dnf
        );
        assert_eq!(manager_from_probe("OS_ID=alpine\n"), Manager::Apk);
        assert_eq!(
            manager_from_probe("OS_ID=unknown\nHAS_pacman=1"),
            Manager::Pacman
        );
        assert_eq!(manager_from_probe("OS_ID=unknown\n"), Manager::Unknown);
    }

    #[test]
    fn every_known_manager_has_both_privileged_commands() {
        for manager in [
            Manager::Apt,
            Manager::Dnf,
            Manager::Yum,
            Manager::Zypper,
            Manager::Pacman,
            Manager::Apk,
        ] {
            let install = manager.install_command().unwrap();
            let upgrade = manager.upgrade_command().unwrap();
            let start = manager.start_command().unwrap();
            assert!(!install.is_empty(), "{:?}", manager);
            assert!(!upgrade.is_empty(), "{:?}", manager);
            assert!(!start.is_empty(), "{:?}", manager);
        }
        assert!(Manager::Unknown.install_command().is_none());
        assert!(Manager::Unknown.upgrade_command().is_none());
        assert!(Manager::Unknown.start_command().is_none());
    }

    #[test]
    fn the_probe_succeeds_whatever_tools_it_finds() {
        // The last lookup fails for any tool the host lacks, and a non-zero status would be
        // read as a failed probe while the answer itself arrived intact.
        assert!(
            DETECT_COMMAND.trim_end().ends_with("true"),
            "{DETECT_COMMAND}"
        );
    }

    #[test]
    fn the_windows_plan_installs_docker_desktop_elevated() {
        let command = windows_install_command();
        assert!(command.contains(DOCKER_DESKTOP_PACKAGE), "{command}");
        assert!(command.contains("-Verb RunAs"), "{command}");
        // Waiting for the installer would outlast the command timeout.
        assert!(!command.contains("-Wait"), "{command}");
    }

    #[test]
    fn the_engine_status_reads_docker_info() {
        let status = parse_info(
            r#"{"ServerVersion":"27.1.1","OperatingSystem":"Debian GNU/Linux 12 (bookworm)",
                "Architecture":"x86_64","Containers":5,"ContainersRunning":3,
                "ContainersPaused":1,"ContainersStopped":1,"Images":12}"#,
        )
        .unwrap();
        assert!(status.is_ready());
        assert_eq!(status.version, "27.1.1");
        assert_eq!(status.running, 3);
        assert_eq!(status.paused, 1);
        assert_eq!(status.stopped, 1);
        assert_eq!(status.images, 12);
        assert!(
            status.summary().contains("Engine 27.1.1"),
            "{}",
            status.summary()
        );
    }

    #[test]
    fn an_answer_without_a_version_is_not_an_engine() {
        assert!(parse_info("{}").is_none());
        assert!(parse_info(r#"{"ContainersRunning":0,"Images":0}"#).is_none());
        assert!(parse_info("not json").is_none());
        let status = Status {
            state: Some(State::Error),
            message: UNEXPECTED_ANSWER.to_owned(),
            ..Status::default()
        };
        assert!(!status.is_ready());
        assert!(
            status.summary().contains("Docker engine version"),
            "{}",
            status.summary()
        );
    }

    #[test]
    fn failures_are_told_apart() {
        assert_eq!(
            status_from_failure(
                "permission denied while trying to connect to the Docker daemon socket"
            )
            .state,
            Some(State::Forbidden)
        );
        assert_eq!(
            status_from_failure(
                "Cannot connect to the Docker daemon at unix:///var/run/docker.sock"
            )
            .state,
            Some(State::Unavailable)
        );
        assert_eq!(
            status_from_failure("sh: 1: docker: not found").state,
            Some(State::Missing)
        );
        assert_eq!(
            status_from_failure(
                "docker : The term 'docker' is not recognized as the name of a cmdlet"
            )
            .state,
            Some(State::Missing)
        );
        assert_eq!(status_from_failure("boom").state, Some(State::Error));
    }
}
