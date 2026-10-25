//! Saved hosts and their organisation (`hosts.json`).

use serde::{Deserialize, Serialize};

use crate::connection::ConnectionParams;
use crate::settings::PluginSettings;
use crate::theme::Rgba;

/// A sidebar group.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostGroup {
    pub id: String,
    pub name: String,
    #[serde(default, deserialize_with = "crate::theme::deserialize_optional_color")]
    pub color: Option<Rgba>,
    #[serde(default)]
    pub collapsed: bool,
}

/// A saved host profile.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostProfile {
    pub id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub group_id: Option<String>,
    pub connection: ConnectionParams,
    /// Per-plugin settings shown in Host settings for this host.
    #[serde(default)]
    pub plugin_settings: PluginSettings,
}

impl HostProfile {
    /// Name shown in the sidebar; falls back to the endpoint when unnamed.
    pub fn display_name(&self) -> &str {
        if self.name.trim().is_empty() {
            self.connection.host.as_str()
        } else {
            self.name.as_str()
        }
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|existing| existing == tag)
    }

    /// Add a tag, ignoring empty values and case-insensitive duplicates.
    pub fn add_tag(&mut self, tag: &str) {
        let tag = tag.trim();
        if tag.is_empty() {
            return;
        }
        if !self.tags.iter().any(|existing| existing.eq_ignore_ascii_case(tag)) {
            self.tags.push(tag.to_owned());
        }
    }
}

/// The whole `hosts.json` document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct HostsDocument {
    pub groups: Vec<HostGroup>,
    /// Ids of hosts shown outside any group, in sidebar order.
    pub ungrouped_host_ids: Vec<String>,
    pub ungrouped_collapsed: bool,
    pub hosts: Vec<HostProfile>,
}

impl HostsDocument {
    /// Remove duplicates and dangling references, then normalise every connection.
    pub fn normalized(mut self) -> Self {
        let mut seen_hosts = std::collections::HashSet::new();
        self.hosts.retain(|host| !host.id.is_empty() && seen_hosts.insert(host.id.clone()));

        let mut seen_groups = std::collections::HashSet::new();
        self.groups
            .retain(|group| !group.id.is_empty() && seen_groups.insert(group.id.clone()));

        let known_group_ids: std::collections::HashSet<_> =
            self.groups.iter().map(|group| group.id.clone()).collect();
        for host in &mut self.hosts {
            let group_id = host.group_id.clone();
            if let Some(group_id) = group_id
                && !known_group_ids.contains(&group_id)
            {
                host.group_id = None;
            }
            host.connection = host.connection.clone().normalized();
            host.tags.retain(|tag| !tag.trim().is_empty());
            host.plugin_settings.retain(|_, values| !values.is_empty());
        }

        let host_ids: std::collections::HashSet<_> =
            self.hosts.iter().map(|host| host.id.clone()).collect();
        self.ungrouped_host_ids
            .retain(|id| host_ids.contains(id));
        self.ungrouped_host_ids.dedup();
        // Ungrouped hosts missing from the order list are appended in document order.
        for host in &self.hosts {
            if host.group_id.is_none() && !self.ungrouped_host_ids.contains(&host.id) {
                self.ungrouped_host_ids.push(host.id.clone());
            }
        }
        // A grouped host must not also appear in the ungrouped order list.
        let grouped: std::collections::HashSet<_> = self
            .hosts
            .iter()
            .filter(|host| host.group_id.is_some())
            .map(|host| host.id.clone())
            .collect();
        self.ungrouped_host_ids
            .retain(|id| !grouped.contains(id));
        self
    }

    pub fn host(&self, id: &str) -> Option<&HostProfile> {
        self.hosts.iter().find(|host| host.id == id)
    }

    pub fn host_mut(&mut self, id: &str) -> Option<&mut HostProfile> {
        self.hosts.iter_mut().find(|host| host.id == id)
    }

    /// Hosts belonging to a group, in document order.
    pub fn hosts_in_group(&self, group_id: &str) -> Vec<&HostProfile> {
        self.hosts
            .iter()
            .filter(|host| host.group_id.as_deref() == Some(group_id))
            .collect()
    }

    /// Hosts shown in the ungrouped section, honouring the saved order.
    pub fn ungrouped(&self) -> Vec<&HostProfile> {
        self.ungrouped_host_ids
            .iter()
            .filter_map(|id| self.host(id))
            .collect()
    }

    /// Add or replace a host and keep the ungrouped order consistent.
    pub fn upsert(&mut self, host: HostProfile) {
        match self.hosts.iter_mut().find(|existing| existing.id == host.id) {
            Some(existing) => *existing = host,
            None => self.hosts.push(host),
        }
        let ungrouped: Vec<String> = self
            .hosts
            .iter()
            .filter(|host| host.group_id.is_none())
            .map(|host| host.id.clone())
            .collect();
        self.ungrouped_host_ids
            .retain(|id| ungrouped.contains(id));
        for id in ungrouped {
            if !self.ungrouped_host_ids.contains(&id) {
                self.ungrouped_host_ids.push(id);
            }
        }
    }

    /// Remove a host, its organisation entries and any jump-host references to it.
    pub fn remove(&mut self, id: &str) -> Option<HostProfile> {
        let index = self.hosts.iter().position(|host| host.id == id)?;
        let removed = self.hosts.remove(index);
        self.ungrouped_host_ids.retain(|existing| existing != id);
        for host in &mut self.hosts {
            if host.connection.jump_host_id.as_deref() == Some(id) {
                host.connection.jump_host_id = None;
            }
        }
        Some(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::ConnectionType;

    fn host(id: &str, group: Option<&str>) -> HostProfile {
        HostProfile {
            id: id.to_owned(),
            name: format!("host {id}"),
            group_id: group.map(str::to_owned),
            ..Default::default()
        }
    }

    #[test]
    fn normalization_removes_dangling_group_references() {
        let doc = HostsDocument {
            groups: vec![HostGroup {
                id: "g1".to_owned(),
                name: "Prod".to_owned(),
                color: None,
                collapsed: false,
            }],
            hosts: vec![host("a", Some("missing")), host("b", Some("g1"))],
            ..Default::default()
        }
        .normalized();
        assert_eq!(doc.host("a").unwrap().group_id, None);
        assert_eq!(doc.host("b").unwrap().group_id.as_deref(), Some("g1"));
        assert_eq!(doc.ungrouped_host_ids, vec!["a".to_owned()]);
    }

    #[test]
    fn duplicates_are_dropped() {
        let doc = HostsDocument {
            hosts: vec![host("a", None), host("a", None)],
            groups: vec![
                HostGroup {
                    id: "g".to_owned(),
                    name: "one".to_owned(),
                    color: None,
                    collapsed: false,
                },
                HostGroup {
                    id: "g".to_owned(),
                    name: "two".to_owned(),
                    color: None,
                    collapsed: false,
                },
            ],
            ..Default::default()
        }
        .normalized();
        assert_eq!(doc.hosts.len(), 1);
        assert_eq!(doc.groups.len(), 1);
    }

    #[test]
    fn ungrouped_order_is_preserved_and_completed() {
        let doc = HostsDocument {
            hosts: vec![host("a", None), host("b", None), host("c", None)],
            ungrouped_host_ids: vec!["b".to_owned()],
            ..Default::default()
        }
        .normalized();
        assert_eq!(
            doc.ungrouped_host_ids,
            vec!["b".to_owned(), "a".to_owned(), "c".to_owned()]
        );
    }

    #[test]
    fn grouped_hosts_leave_the_ungrouped_list() {
        let doc = HostsDocument {
            groups: vec![HostGroup {
                id: "g".to_owned(),
                name: "G".to_owned(),
                color: None,
                collapsed: false,
            }],
            hosts: vec![host("a", None), host("b", Some("g"))],
            ungrouped_host_ids: vec!["a".to_owned(), "b".to_owned()],
            ..Default::default()
        }
        .normalized();
        assert_eq!(doc.ungrouped_host_ids, vec!["a".to_owned()]);
    }

    #[test]
    fn removing_a_host_clears_jump_host_references() {
        let mut doc = HostsDocument {
            hosts: vec![host("jump", None), host("target", None)],
            ..Default::default()
        }
        .normalized();
        doc.host_mut("target").unwrap().connection.jump_host_id = Some("jump".to_owned());
        let removed = doc.remove("jump").expect("removed");
        assert_eq!(removed.id, "jump");
        assert_eq!(doc.host("target").unwrap().connection.jump_host_id, None);
        assert!(doc.host("jump").is_none());
    }

    #[test]
    fn connection_defaults_are_normalized_on_read() {
        let doc = HostsDocument {
            hosts: vec![HostProfile {
                id: "h".to_owned(),
                name: "H".to_owned(),
                connection: ConnectionParams {
                    host: "example.com".to_owned(),
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        }
        .normalized();
        assert_eq!(doc.host("h").unwrap().connection.port, 22);
        assert_eq!(doc.host("h").unwrap().connection.kind, ConnectionType::Ssh);
    }

    #[test]
    fn tags_are_unique_and_trimmed() {
        let mut host = HostProfile::default();
        host.add_tag(" prod ");
        host.add_tag("PROD");
        host.add_tag("");
        assert_eq!(host.tags, vec!["prod".to_owned()]);
        assert!(host.has_tag("prod"));
    }

    #[test]
    fn json_round_trip() {
        let doc = HostsDocument {
            groups: vec![HostGroup {
                id: "g".to_owned(),
                name: "Prod".to_owned(),
                color: Some(Rgba::rgb(0xe35d6a)),
                collapsed: true,
            }],
            hosts: vec![host("a", Some("g"))],
            ungrouped_collapsed: true,
            ..Default::default()
        };
        let json = serde_json::to_string(&doc).unwrap();
        assert!(json.contains("\"color\":\"#e35d6a\""));
        let back: HostsDocument = serde_json::from_str(&json).unwrap();
        assert_eq!(back, doc);
    }
}
