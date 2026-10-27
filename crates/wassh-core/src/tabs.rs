//! Open tabs (`tabs.json`).

use serde::{Deserialize, Serialize};

use crate::connection::ConnectionParams;
use crate::layout::TabPluginLayout;

/// One open tab as persisted between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TabSnapshot {
    pub id: String,
    pub connection: ConnectionParams,
    /// Saved host this tab was opened from, so a restart keeps the link.
    pub host_id: Option<String>,
    pub active: bool,
    pub active_plugin_ids: Vec<String>,
    /// Dock layout for this tab, absent when no plugin pane was open.
    pub plugin_layout: Option<TabPluginLayout>,
}

impl TabSnapshot {
    pub fn normalized(mut self) -> Self {
        self.active_plugin_ids.sort();
        self.active_plugin_ids.dedup();
        self.connection = self.connection.normalized();
        self
    }
}

/// Drop duplicate ids and guarantee exactly one active tab when any exist.
pub fn normalize_tabs(mut tabs: Vec<TabSnapshot>) -> Vec<TabSnapshot> {
    let mut seen = std::collections::HashSet::new();
    tabs.retain(|tab| !tab.id.is_empty() && seen.insert(tab.id.clone()));
    for tab in &mut tabs {
        *tab = tab.clone().normalized();
    }

    let mut found_active = false;
    for tab in &mut tabs {
        if tab.active {
            if found_active {
                tab.active = false;
            } else {
                found_active = true;
            }
        }
    }
    if !found_active && let Some(first) = tabs.first_mut() {
        first.active = true;
    }
    tabs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::ConnectionType;

    fn tab(id: &str, active: bool) -> TabSnapshot {
        TabSnapshot {
            id: id.to_owned(),
            active,
            ..Default::default()
        }
    }

    #[test]
    fn empty_list_stays_empty() {
        assert!(normalize_tabs(Vec::new()).is_empty());
    }

    #[test]
    fn first_tab_becomes_active_when_none_is() {
        let tabs = normalize_tabs(vec![tab("a", false), tab("b", false)]);
        assert!(tabs[0].active);
        assert!(!tabs[1].active);
    }

    #[test]
    fn only_one_tab_stays_active() {
        let tabs = normalize_tabs(vec![tab("a", true), tab("b", true), tab("c", false)]);
        assert!(tabs[0].active);
        assert!(!tabs[1].active);
    }

    #[test]
    fn duplicate_and_blank_ids_are_dropped() {
        let tabs = normalize_tabs(vec![tab("a", false), tab("a", true), tab("", false)]);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, "a");
        assert!(tabs[0].active);
    }

    #[test]
    fn plugin_ids_are_deduplicated() {
        let mut tab = tab("a", true);
        tab.active_plugin_ids = vec!["sftp".into(), "docker-manager".into(), "sftp".into()];
        let tab = tab.normalized();
        assert_eq!(tab.active_plugin_ids, vec!["docker-manager", "sftp"]);
    }

    #[test]
    fn the_host_link_is_persisted() {
        let tab = TabSnapshot {
            id: "t".to_owned(),
            host_id: Some("host-1".to_owned()),
            ..TabSnapshot::default()
        };
        let json = serde_json::to_string(&tab).unwrap();
        assert!(json.contains("\"hostId\":\"host-1\""), "{json}");
        let back: TabSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.host_id.as_deref(), Some("host-1"));
    }

    #[test]
    fn json_round_trip() {
        let tabs = vec![TabSnapshot {
            id: "t".to_owned(),
            connection: ConnectionParams {
                kind: ConnectionType::Telnet,
                host: "example.com".to_owned(),
                port: 23,
                ..Default::default()
            },
            active: true,
            active_plugin_ids: vec!["scratchpad".to_owned()],
            host_id: Some("h1".to_owned()),
            plugin_layout: Some(TabPluginLayout {
                left: Some(crate::layout::LayoutNode::leaf("scratchpad")),
                ..TabPluginLayout::empty()
            }),
        }];
        let json = serde_json::to_string(&tabs).unwrap();
        assert!(json.contains("\"activePluginIds\""));
        assert!(json.contains("\"pluginLayout\""));
        let back: Vec<TabSnapshot> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tabs);
    }
}
