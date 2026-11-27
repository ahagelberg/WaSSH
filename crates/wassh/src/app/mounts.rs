//! Remote folders, mounted as local drives.
//!
//! The app owns these, not a tab: a drive is a property of the machine, not of the window it
//! was started from. A mount resolves its host the way a tab does (credentials, jump hosts,
//! host keys), opens its own SFTP channel, and hands that to the filesystem the platform
//! serves. When the connection goes away the filesystem stays mounted and a fresh client is
//! swapped in once the host is reachable again.
//!
//! Only Windows can mount in this build; everywhere else the same interface answers that
//! mounting needs a platform it does not have.

pub use imp::Mounts;

/// Everything a mount needs, on a platform that has a filesystem driver.
#[cfg(windows)]
mod imp {
    use std::collections::BTreeMap;
    use std::path::Path;
    use std::process::Command;
    use std::sync::Arc;
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    use serde::{Deserialize, Serialize};
    use wassh_core::{HostsDocument, Identity};
    use wassh_mount::windows::Mount;
    use wassh_mount::SftpFs;
    use wassh_plugins::{
        DriverState, MountState, MountedDrive, RemoteHost, RemoteMounts, RememberedMount,
    };
    use wassh_ssh::{HostKeyPolicy, KnownHosts, SftpClient, SshAuth, SshEvent, SshOptions, SshSession};
    use wassh_store::JsonDocument;

    use super::super::reach::{Inventory, Reach};
    use super::super::session::Secrets;

    /// First wait before a lost mount dials again.
    const RETRY_FIRST: Duration = Duration::from_secs(2);
    /// Longest wait between attempts.
    const RETRY_MAX: Duration = Duration::from_secs(30);
    /// How long a host that does not keep retrying keeps a failing mount before it gives up.
    const GIVE_UP_AFTER: Duration = Duration::from_secs(5 * 60);
    /// Drive letters a mount may use, from the first one Windows leaves for extra disks.
    const FIRST_LETTER: char = 'D';
    const LAST_LETTER: char = 'Z';

    /// One mount the user asked for, as it was saved.
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct SavedMount {
        /// Host the folder is on.
        pub key: String,
        /// Saved host id.
        pub host_id: String,
        /// Display name, so a stale entry still reads well.
        pub host_name: String,
        /// Folder that is served.
        pub remote_path: String,
        /// Where it was mounted.
        pub mount_point: String,
    }

    /// `mounts.json`: the mounts to offer again next time.
    #[derive(Clone, Debug, Default, Deserialize, Serialize)]
    #[serde(rename_all = "camelCase", default)]
    pub struct MountsDocument {
        pub mounts: Vec<SavedMount>,
    }

    /// One mount, from the click that asked for it to the drive it becomes.
    struct Live {
        key: String,
        host_id: String,
        host_name: String,
        remote_path: String,
        mount_point: String,
        state: MountState,
        /// The filesystem, once there has been a first connection.
        fs: Option<Arc<SftpFs>>,
        mount: Option<Mount>,
        session: Option<SshSession>,
        /// Answer from [`SshSession::open_sftp_client`], until the client arrives.
        pending: Option<Receiver<Result<SftpClient, String>>>,
        /// Password the user typed for this mount; never written anywhere.
        password: Option<String>,
        attempts: u32,
        next_attempt: Instant,
        /// When the connection was first lost, for the give-up rule.
        lost_at: Option<Instant>,
        reconnects: u32,
    }

    impl Live {
        fn new(key: String, host_id: String, host_name: String, remote_path: String, mount_point: String) -> Self {
            Self {
                key,
                host_id,
                host_name,
                remote_path,
                mount_point,
                state: MountState::Mounting,
                fs: None,
                mount: None,
                session: None,
                pending: None,
                password: None,
                attempts: 0,
                next_attempt: Instant::now(),
                lost_at: None,
                reconnects: 0,
            }
        }

        /// How the pane sees it.
        fn drive(&self) -> MountedDrive {
            MountedDrive {
                key: self.key.clone(),
                mount_point: self.mount_point.clone(),
                host_id: self.host_id.clone(),
                host_name: self.host_name.clone(),
                remote_path: self.remote_path.clone(),
                state: self.state.clone(),
                reconnects: self.reconnects,
            }
        }
    }

    /// Every mount the app knows about.
    pub struct Mounts {
        inventory: Inventory,
        known_hosts: Arc<KnownHosts>,
        live: BTreeMap<String, Live>,
        saved: Vec<SavedMount>,
        document: Option<JsonDocument<MountsDocument>>,
        /// Per-host answer to "keep a failing mount trying".
        keep_retrying: BTreeMap<String, bool>,
    }

    impl Mounts {
        /// A broker that checks host keys against `known_hosts` and remembers mounts in
        /// `mounts.json` under the app's data directory.
        pub fn new(known_hosts: Arc<KnownHosts>) -> Self {
            let (document, saved) = match wassh_store::data_dir() {
                Ok(dir) => {
                    let (document, problem) = JsonDocument::open_lenient(
                        wassh_store::paths::mounts_file(dir),
                        MountsDocument::default(),
                        |document| document,
                    );
                    if let Some(problem) = problem {
                        eprintln!("mounts: {problem}");
                    }
                    let saved = document.get().mounts.clone();
                    (Some(document), saved)
                }
                Err(_) => (None, Vec::new()),
            };
            Self {
                inventory: Inventory::new(),
                known_hosts,
                live: BTreeMap::new(),
                saved,
                document,
                keep_retrying: BTreeMap::new(),
            }
        }

        /// Rebuild what the broker knows about the saved hosts.
        pub fn sync(
            &mut self,
            hosts: &HostsDocument,
            identities: &[Identity],
            vault: Option<&wassh_store::SecretsStore>,
        ) {
            self.inventory
                .sync(hosts, identities, vault, &self.known_hosts);
            self.keep_retrying.clear();
            for host in &hosts.hosts {
                let keep = wassh_core::plugin_setting_values(&host.plugin_settings, "mounts")
                    .and_then(|values| values.get("keepRetrying"))
                    .and_then(|value| value.as_bool())
                    .unwrap_or(true);
                self.keep_retrying.insert(host.id.clone(), keep);
            }
        }

        /// Dial a mount's host, or record why it cannot be dialled.
        fn dial(&mut self, key: &str) {
            let Some(live) = self.live.get_mut(key) else {
                return;
            };
            let Some(reach) = self.inventory.reach(&live.host_id) else {
                live.state = MountState::Failed(format!("'{}' is not a saved host", live.host_id));
                return;
            };
            let (params, secrets, jumps) = match reach {
                Reach::Blocked(reason) => {
                    live.state = MountState::Failed(reason.clone());
                    return;
                }
                Reach::NeedsPassword { params, jumps } => match live.password.clone() {
                    Some(password) => (
                        params.as_ref().clone(),
                        Secrets {
                            password,
                            key_passphrase: String::new(),
                        },
                        jumps.clone(),
                    ),
                    None => {
                        live.state = MountState::NeedsPassword;
                        return;
                    }
                },
                Reach::Ready {
                    params,
                    secrets,
                    jumps,
                } => (params.as_ref().clone(), secrets.clone(), jumps.clone()),
            };
            let options = mount_options(&params, &secrets, jumps, Arc::clone(&self.known_hosts));
            live.attempts += 1;
            match SshSession::connect(options) {
                Ok(session) => {
                    live.session = Some(session);
                    live.state = MountState::Mounting;
                }
                Err(error) => {
                    live.state = MountState::Failed(format!("{error:#}"));
                }
            }
        }

        /// Take in what every mount's connection has produced, and keep the retries moving.
        pub fn poll(&mut self) {
            let now = Instant::now();
            let keys: Vec<String> = self.live.keys().cloned().collect();
            for key in keys {
                let mut lost = false;
                if let Some(live) = self.live.get_mut(&key) {
                    if let Some(session) = live.session.as_ref() {
                        for event in session.events() {
                            match event {
                                SshEvent::Status {
                                    status: wassh_core::SessionStatus::Connected,
                                    ..
                                } => {
                                    live.pending = Some(session.open_sftp_client());
                                }
                                SshEvent::Status {
                                    status:
                                        wassh_core::SessionStatus::Disconnected
                                        | wassh_core::SessionStatus::Closed
                                        | wassh_core::SessionStatus::Failed,
                                    ..
                                } => {
                                    lost = true;
                                }
                                SshEvent::Closed { .. } | SshEvent::Error(_) => lost = true,
                                _ => {}
                            }
                        }
                    }
                    if let Some(answer) = live.pending.as_ref() {
                        match answer.try_recv() {
                            Ok(Ok(client)) => {
                                live.pending = None;
                                self.attach(&key, client);
                            }
                            Ok(Err(error)) => {
                                live.pending = None;
                                live.state = MountState::Failed(error);
                                live.session.take();
                                lost = true;
                            }
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                live.pending = None;
                                lost = true;
                            }
                        }
                    }
                }
                if lost {
                    self.lost(&key);
                }
                self.retry_if_due(&key, now);
            }
        }

        /// Put a freshly connected client behind the mount's filesystem.
        fn attach(&mut self, key: &str, client: SftpClient) {
            let Some(live) = self.live.get_mut(key) else {
                return;
            };
            let label = format!("{} on {}", live.remote_path, live.host_name);
            match live.fs.as_ref() {
                Some(fs) => {
                    fs.replace_client(client);
                    live.reconnects += 1;
                }
                None => {
                    let fs = Arc::new(SftpFs::new(client));
                    match Mount::start(fs.clone(), &label, &live.mount_point) {
                        Ok(mount) => {
                            live.fs = Some(fs);
                            live.mount = Some(mount);
                            live.reconnects = 0;
                        }
                        Err(error) => {
                            live.state = MountState::Failed(error);
                            live.session.take();
                            return;
                        }
                    }
                }
            }
            live.state = MountState::Open;
            live.attempts = 0;
            live.lost_at = None;
            let saved = SavedMount {
                key: live.key.clone(),
                host_id: live.host_id.clone(),
                host_name: live.host_name.clone(),
                remote_path: live.remote_path.clone(),
                mount_point: live.mount_point.clone(),
            };
            self.remember(saved);
        }

        /// Note that a mount's connection went away and schedule its next attempt.
        fn lost(&mut self, key: &str) {
            let keep = self
                .live
                .get(key)
                .map(|live| self.keep_retrying.get(&live.host_id).copied().unwrap_or(true))
                .unwrap_or(true);
            let Some(live) = self.live.get_mut(key) else {
                return;
            };
            if let Some(mut session) = live.session.take() {
                session.disconnect();
            }
            live.pending = None;
            live.mount = None;
            if let Some(fs) = live.fs.as_ref() {
                fs.clear_client();
            }
            let now = Instant::now();
            let lost_at = *live.lost_at.get_or_insert(now);
            if !keep && now.duration_since(lost_at) > GIVE_UP_AFTER {
                live.state =
                    MountState::Failed("the connection kept failing; unmounted".to_owned());
                live.fs = None;
                return;
            }
            live.state = MountState::Lost;
            let wait = RETRY_FIRST
                .saturating_mul(2u32.saturating_pow(live.attempts.min(4)))
                .min(RETRY_MAX);
            live.next_attempt = now + wait;
        }

        /// Dial again when a lost mount's wait is over.
        fn retry_if_due(&mut self, key: &str, now: Instant) {
            let due = self
                .live
                .get(key)
                .map(|live| {
                    live.session.is_none()
                        && matches!(live.state, MountState::Lost | MountState::Mounting)
                        && now >= live.next_attempt
                })
                .unwrap_or(false);
            if due {
                self.dial(key);
            }
        }

        /// Add `saved` to the document, replacing an entry with the same key.
        fn remember(&mut self, saved: SavedMount) {
            self.saved.retain(|entry| entry.key != saved.key);
            self.saved.push(saved);
            if let Some(document) = self.document.as_mut() {
                let mounts = self.saved.clone();
                let _ = document.update(|value| value.mounts = mounts);
            }
        }

        /// Drop a saved entry.
        fn forget_saved(&mut self, key: &str) {
            self.saved.retain(|entry| entry.key != key);
            if let Some(document) = self.document.as_mut() {
                let mounts = self.saved.clone();
                let _ = document.update(|value| value.mounts = mounts);
            }
        }

        /// Stop a mount: the drive goes, the session is dropped.
        fn stop(&mut self, key: &str) {
            if let Some(mut live) = self.live.remove(key) {
                live.mount = None;
                if let Some(fs) = live.fs.as_ref() {
                    fs.clear_client();
                }
                if let Some(mut session) = live.session.take() {
                    session.disconnect();
                }
            }
        }

        /// Mount points nothing is using.
        fn free_points(&self) -> Vec<String> {
            let used: Vec<&str> = self
                .live
                .values()
                .map(|live| live.mount_point.as_str())
                .collect();
            (FIRST_LETTER..=LAST_LETTER)
                .map(|letter| format!("{letter}:"))
                .filter(|point| !used.contains(&point.as_str()))
                .filter(|point| !Path::new(&format!("{point}\\")).exists())
                .collect()
        }
    }

    impl RemoteMounts for Mounts {
        fn mounts(&self) -> Vec<MountedDrive> {
            self.live.values().map(Live::drive).collect()
        }

        fn hosts(&self) -> &[RemoteHost] {
            self.inventory.hosts()
        }

        fn free_drives(&self) -> Vec<String> {
            self.free_points()
        }

        fn driver(&self) -> DriverState {
            match wassh_mount::windows::driver_state() {
                wassh_mount::windows::DriverState::Ready { .. } => DriverState::Ready,
                wassh_mount::windows::DriverState::Missing { reason } => {
                    DriverState::Missing(reason)
                }
            }
        }

        fn mount(&mut self, key: &str, host_id: &str, remote_path: &str, mount_point: &str) {
            if self.live.contains_key(key) {
                return;
            }
            let host_name = self
                .inventory
                .hosts()
                .iter()
                .find(|host| host.id == host_id)
                .map(|host| host.name.clone())
                .unwrap_or_else(|| host_id.to_owned());
            self.live.insert(
                key.to_owned(),
                Live::new(
                    key.to_owned(),
                    host_id.to_owned(),
                    host_name,
                    remote_path.to_owned(),
                    mount_point.to_owned(),
                ),
            );
            self.dial(key);
        }

        fn unmount(&mut self, key: &str) {
            self.stop(key);
            self.forget_saved(key);
        }

        fn retry(&mut self, key: &str) {
            if let Some(live) = self.live.get_mut(key) {
                live.lost_at = None;
                live.attempts = 0;
                live.next_attempt = Instant::now();
                live.state = MountState::Lost;
            }
            self.dial(key);
        }

        fn supply_password(&mut self, key: &str, password: &str) {
            if let Some(live) = self.live.get_mut(key) {
                live.password = Some(password.to_owned());
            }
            self.dial(key);
        }

        fn reveal(&mut self, mount_point: &str) {
            let _ = Command::new("explorer.exe").arg(mount_point).spawn();
        }

        fn install_driver(&mut self) {
            let _ = Command::new("winget")
                .args([
                    "install",
                    "--id",
                    "WinFsp.WinFsp",
                    "-e",
                    "--accept-package-agreements",
                    "--accept-source-agreements",
                ])
                .spawn();
        }

        fn remembered(&self) -> Vec<RememberedMount> {
            self.saved
                .iter()
                .filter(|entry| !self.live.contains_key(&entry.key))
                .map(|entry| RememberedMount {
                    key: entry.key.clone(),
                    host_id: entry.host_id.clone(),
                    host_name: entry.host_name.clone(),
                    remote_path: entry.remote_path.clone(),
                    mount_point: entry.mount_point.clone(),
                    problem: (!self.inventory.contains(&entry.host_id))
                        .then(|| "host was deleted".to_owned()),
                })
                .collect()
        }

        fn remount(&mut self, key: &str) {
            let Some(saved) = self.saved.iter().find(|entry| entry.key == key).cloned() else {
                return;
            };
            let point = if Path::new(&format!("{}\\", saved.mount_point)).exists()
                && !self.free_points().contains(&saved.mount_point)
            {
                match self.free_points().into_iter().next() {
                    Some(point) => point,
                    None => return,
                }
            } else {
                saved.mount_point.clone()
            };
            self.mount(key, &saved.host_id, &saved.remote_path, &point);
        }

        fn forget(&mut self, key: &str) {
            self.stop(key);
            self.forget_saved(key);
        }
    }

    /// Transport options for a mount: its own connection, no shell, no prompts.
    fn mount_options(
        params: &wassh_core::ConnectionParams,
        secrets: &Secrets,
        jumps: Vec<wassh_ssh::JumpHop>,
        known_hosts: Arc<KnownHosts>,
    ) -> SshOptions {
        let auth = match params.auth {
            wassh_core::AuthMethod::Password => SshAuth::Password(secrets.password.clone()),
            wassh_core::AuthMethod::PrivateKey => SshAuth::PrivateKey {
                path: params.private_key_path.clone().unwrap_or_default().into(),
                passphrase: (!secrets.key_passphrase.is_empty())
                    .then(|| secrets.key_passphrase.clone()),
            },
            wassh_core::AuthMethod::None => SshAuth::Agent,
        };
        let mut options = SshOptions::new(params.host.clone(), params.username.clone(), auth);
        options.port = params.port;
        options.jumps = jumps;
        options.shell = false;
        options.host_key_policy = HostKeyPolicy::Known(known_hosts);
        options
    }
}

/// Everything a mount needs, on a platform without a filesystem driver.
#[cfg(not(windows))]
mod imp {
    use wassh_core::{HostsDocument, Identity};
    use wassh_plugins::{DriverState, MountedDrive, RemoteHost, RemoteMounts, RememberedMount};

    /// A broker that cannot mount anything, because this platform has no driver yet.
    pub struct Mounts;

    impl Mounts {
        /// A broker for a platform that cannot mount.
        pub fn new(_known_hosts: std::sync::Arc<wassh_ssh::KnownHosts>) -> Self {
            Self
        }

        /// Nothing to rebuild.
        pub fn sync(
            &mut self,
            _hosts: &HostsDocument,
            _identities: &[Identity],
            _vault: Option<&wassh_store::SecretsStore>,
        ) {
        }

        /// Nothing to poll.
        pub fn poll(&mut self) {}
    }

    impl RemoteMounts for Mounts {
        fn mounts(&self) -> Vec<MountedDrive> {
            Vec::new()
        }

        fn hosts(&self) -> &[RemoteHost] {
            &[]
        }

        fn free_drives(&self) -> Vec<String> {
            Vec::new()
        }

        fn driver(&self) -> DriverState {
            DriverState::Missing("Mounting a remote folder is only supported on Windows.".to_owned())
        }

        fn mount(&mut self, _key: &str, _host_id: &str, _remote_path: &str, _mount_point: &str) {}

        fn unmount(&mut self, _key: &str) {}

        fn retry(&mut self, _key: &str) {}

        fn supply_password(&mut self, _key: &str, _password: &str) {}

        fn reveal(&mut self, _mount_point: &str) {}

        fn install_driver(&mut self) {}

        fn remembered(&self) -> Vec<RememberedMount> {
            Vec::new()
        }

        fn remount(&mut self, _key: &str) {}

        fn forget(&mut self, _key: &str) {}
    }
}
