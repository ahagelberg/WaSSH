//! What a saved host needs before a connection to it can be dialled.
//!
//! Both brokers that open hosts the user is not sitting in front of - the side connections a
//! pane asks for, and the folders it mounts - resolve credentials, jump hosts and host keys
//! the same way a tab does. This is that resolution, kept in one place.

use std::collections::HashMap;

use wassh_core::{AuthMethod, ConnectionParams, HostProfile, HostsDocument, Identity};
use wassh_plugins::RemoteHost;
use wassh_ssh::{JumpHop, KnownHosts};

use super::session::{Secrets, jump_chain, resolve_auth};

/// Port an SSH connection uses when the profile leaves it unset.
const DEFAULT_PORT: u16 = 22;

/// What one saved host needs before it can be dialled.
pub enum Reach {
    /// Ready: parameters, the secrets they need, and the jump hosts in front of it.
    Ready {
        params: Box<ConnectionParams>,
        secrets: Secrets,
        jumps: Vec<JumpHop>,
    },
    /// The parameters need a password the vault does not hold.
    NeedsPassword {
        params: Box<ConnectionParams>,
        jumps: Vec<JumpHop>,
    },
    /// It cannot be opened, and this is why.
    Blocked(String),
}

/// The saved hosts, and what dialling each of them would take.
pub struct Inventory {
    hosts: Vec<RemoteHost>,
    reach: HashMap<String, Reach>,
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

impl Inventory {
    /// An inventory of nothing.
    pub fn new() -> Self {
        Self {
            hosts: Vec::new(),
            reach: HashMap::new(),
        }
    }

    /// Rebuild from the app's configuration.
    ///
    /// Called when the host list may have changed, not per frame: it resolves every saved
    /// host's credentials and jump chain once, so a request costs nothing.
    pub fn sync(
        &mut self,
        hosts: &HostsDocument,
        identities: &[Identity],
        vault: Option<&wassh_store::SecretsStore>,
        known_hosts: &KnownHosts,
    ) {
        self.hosts.clear();
        self.reach.clear();
        for host in &hosts.hosts {
            let (reach, problem) = classify(host, hosts, identities, vault, known_hosts);
            self.reach.insert(host.id.clone(), reach);
            self.hosts.push(RemoteHost {
                id: host.id.clone(),
                name: host.display_name().to_owned(),
                address: address_of(host),
                tags: host.tags.clone(),
                problem,
            });
        }
    }

    /// Every saved host, in the order the host list shows them, without secrets.
    pub fn hosts(&self) -> &[RemoteHost] {
        &self.hosts
    }

    /// What dialling `host_id` would take, when the host is known.
    pub fn reach(&self, host_id: &str) -> Option<&Reach> {
        self.reach.get(host_id)
    }

    /// Whether `host_id` is still a saved host.
    pub fn contains(&self, host_id: &str) -> bool {
        self.reach.contains_key(host_id)
    }
}

/// What opening `host` would need, and what stands in the way when something does.
fn classify(
    host: &HostProfile,
    hosts: &HostsDocument,
    identities: &[Identity],
    vault: Option<&wassh_store::SecretsStore>,
    known_hosts: &KnownHosts,
) -> (Reach, Option<String>) {
    let (params, secrets) = match resolve_auth(&host.connection, Some(&host.id), identities, vault)
    {
        Ok(resolved) => resolved,
        Err(reason) => return (Reach::Blocked(reason.clone()), Some(reason)),
    };
    let jumps = match jump_chain(hosts, &host.connection, identities, vault) {
        Ok(jumps) => jumps,
        Err(reason) => return (Reach::Blocked(reason.clone()), Some(reason)),
    };
    if known_hosts.get(&params.host, params.port).is_none() {
        let reason = "its host key is not trusted yet - connect once from a terminal tab and \
                      accept it"
            .to_owned();
        return (Reach::Blocked(reason.clone()), Some(reason));
    }
    if params.auth == AuthMethod::Password && secrets.password.is_empty() {
        return (
            Reach::NeedsPassword {
                params: Box::new(params),
                jumps,
            },
            Some("needs a password".to_owned()),
        );
    }
    (
        Reach::Ready {
            params: Box::new(params),
            secrets,
            jumps,
        },
        None,
    )
}

/// `user@host:port`, as a person reads an address.
fn address_of(host: &HostProfile) -> String {
    let port = match host.connection.port {
        0 => DEFAULT_PORT,
        port => port,
    };
    match host.connection.username.trim() {
        "" => format!("{}:{port}", host.connection.host),
        user => format!("{user}@{}:{port}", host.connection.host),
    }
}
