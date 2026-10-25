//! Per-tab plugin dock model: placements and size clamping.
//!
//! The renderer keeps every pane mounted and only
//! changes which one is visible; the dock geometry rules below are what make that layout
//! stable when tabs are reordered or panels are dragged.

use serde::{Deserialize, Serialize};

/// Where a plugin view sits relative to the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DockPlacement {
    SplitLeft,
    SplitRight,
    SplitTop,
    SplitBottom,
    Overlay,
}

impl DockPlacement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SplitLeft => "split-left",
            Self::SplitRight => "split-right",
            Self::SplitTop => "split-top",
            Self::SplitBottom => "split-bottom",
            Self::Overlay => "overlay",
        }
    }

    /// Short label used in settings and the placement picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::SplitLeft => "Left",
            Self::SplitRight => "Right",
            Self::SplitTop => "Top",
            Self::SplitBottom => "Bottom",
            Self::Overlay => "Overlay",
        }
    }

    /// Overlay panes do not consume layout space.
    pub fn is_overlay(self) -> bool {
        matches!(self, Self::Overlay)
    }
}

/// Width of the left/right docks and the splitter between dock and terminal.
pub const DOCK_DEFAULT_WIDTH_PX: f32 = 300.0;
/// Height of the top/bottom docks.
pub const DOCK_DEFAULT_HEIGHT_PX: f32 = 160.0;
/// A dock may not shrink below this while the terminal still has room.
pub const MIN_DOCK_SIZE_PX: f32 = 200.0;
/// The terminal keeps at least this much width regardless of the docks.
pub const MIN_TERMINAL_WIDTH_PX: f32 = 200.0;
/// Splitter thickness.
pub const SPLITTER_SIZE_PX: f32 = 4.0;
/// Drop band at the edge of the terminal area.
pub const OUTER_DROP_BAND_PX: f32 = 28.0;
/// Drop band inside an existing dock.
pub const INNER_DROP_BAND_PX: f32 = 36.0;
/// Growing a dock when a second plugin is inserted beside it.
pub const DOCK_GROW_STEP_PX: f32 = 160.0;

/// Clamp a requested dock extent against the space available along that axis.
///
/// Returns `0.0` when the container is too small for both the minimum dock and the
/// minimum terminal; otherwise the extent stays within `MIN_DOCK_SIZE_PX` and
/// `available - MIN_TERMINAL_WIDTH_PX - SPLITTER_SIZE_PX`.
pub fn clamp_dock_extent(desired: f32, available: f32) -> f32 {
    if !desired.is_finite() || !available.is_finite() {
        return 0.0;
    }
    let usable = available - MIN_TERMINAL_WIDTH_PX - SPLITTER_SIZE_PX;
    if usable < MIN_DOCK_SIZE_PX {
        return 0.0;
    }
    desired.clamp(MIN_DOCK_SIZE_PX, usable)
}

/// Clamp a split ratio so both panes keep `min_px`, falling back to a small ratio range
/// when the container is too small to honour the minimum.
pub fn clamp_split_ratio(ratio: f32, total: f32, min_px: f32) -> f32 {
    if !ratio.is_finite() {
        return 0.5;
    }
    let min_ratio = if total > 2.0 * min_px + SPLITTER_SIZE_PX {
        min_px / total
    } else {
        0.01
    };
    let max_ratio = if total > 2.0 * min_px + SPLITTER_SIZE_PX {
        1.0 - min_ratio
    } else {
        0.99
    };
    ratio.clamp(min_ratio, max_ratio)
}

// --- per-tab layout tree --------------------------------------------------

/// How a split arranges its two children.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    /// Children sit side by side.
    Row,
    /// Children are stacked.
    Column,
}

/// The four docked edges a plugin pane can live on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DockEdge {
    Left,
    Right,
    Top,
    Bottom,
}

impl DockEdge {
    /// Every edge, in the order the renderer visits them.
    pub const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Top, Self::Bottom];

    /// Key used in [`TabPluginLayout`].
    pub fn key(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Top => "top",
            Self::Bottom => "bottom",
        }
    }

    /// Whether the dock consumes width (left/right) rather than height.
    pub fn is_vertical(self) -> bool {
        matches!(self, Self::Left | Self::Right)
    }

    /// Zone that inserts on the far outer side of the dock.
    pub fn outer_zone(self) -> LeafZone {
        match self {
            Self::Left => LeafZone::Left,
            Self::Right => LeafZone::Right,
            Self::Top => LeafZone::Top,
            Self::Bottom => LeafZone::Bottom,
        }
    }

    /// Zone that inserts toward the terminal (the inner side of the dock).
    pub fn inner_zone(self) -> LeafZone {
        match self {
            Self::Left => LeafZone::Right,
            Self::Right => LeafZone::Left,
            Self::Top => LeafZone::Bottom,
            Self::Bottom => LeafZone::Top,
        }
    }
}

/// Which half of an existing leaf a new pane is dropped onto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeafZone {
    Left,
    Right,
    Top,
    Bottom,
}

impl LeafZone {
    fn direction(self) -> SplitDirection {
        match self {
            Self::Left | Self::Right => SplitDirection::Row,
            Self::Top | Self::Bottom => SplitDirection::Column,
        }
    }
}

/// One node of a dock's layout tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum LayoutNode {
    /// A single plugin pane.
    #[serde(rename = "leaf")]
    Leaf {
        #[serde(rename = "pluginId")]
        plugin_id: String,
    },
    /// Two children sharing the space.
    #[serde(rename = "split")]
    Split {
        direction: SplitDirection,
        /// Share of the space given to `a`, in `0.0..=1.0`.
        ratio: f32,
        a: Box<LayoutNode>,
        b: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// A leaf for `plugin_id`.
    pub fn leaf(plugin_id: impl Into<String>) -> Self {
        Self::Leaf {
            plugin_id: plugin_id.into(),
        }
    }

    /// Plugin ids in this subtree, left to right.
    pub fn plugin_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect(&mut ids);
        ids
    }

    fn collect(&self, into: &mut Vec<String>) {
        match self {
            Self::Leaf { plugin_id } => into.push(plugin_id.clone()),
            Self::Split { a, b, .. } => {
                a.collect(into);
                b.collect(into);
            }
        }
    }

    /// Remove `plugin_id`, collapsing splits that lose a side.
    fn remove(&self, plugin_id: &str) -> Option<LayoutNode> {
        match self {
            Self::Leaf { plugin_id: id } => (id != plugin_id).then(|| self.clone()),
            Self::Split {
                direction,
                ratio,
                a,
                b,
            } => {
                let left = a.remove(plugin_id);
                let right = b.remove(plugin_id);
                match (left, right) {
                    (None, None) => None,
                    (None, Some(only)) | (Some(only), None) => Some(only),
                    (Some(a), Some(b)) => Some(Self::Split {
                        direction: *direction,
                        ratio: *ratio,
                        a: Box::new(a),
                        b: Box::new(b),
                    }),
                }
            }
        }
    }

    /// Wrap `existing` in a split that puts `incoming` on the given side.
    fn wrap(existing: LayoutNode, incoming: LayoutNode, zone: LeafZone) -> LayoutNode {
        let (a, b) = match zone {
            LeafZone::Left | LeafZone::Top => (incoming, existing),
            LeafZone::Right | LeafZone::Bottom => (existing, incoming),
        };
        LayoutNode::Split {
            direction: zone.direction(),
            ratio: 0.5,
            a: Box::new(a),
            b: Box::new(b),
        }
    }

    /// Replace the leaf for `target` with `mapper`'s result.
    fn map_leaf(&self, target: &str, mapper: &impl Fn(&str) -> LayoutNode) -> LayoutNode {
        match self {
            Self::Leaf { plugin_id } if plugin_id == target => mapper(plugin_id),
            Self::Leaf { .. } => self.clone(),
            Self::Split {
                direction,
                ratio,
                a,
                b,
            } => Self::Split {
                direction: *direction,
                ratio: *ratio,
                a: Box::new(a.map_leaf(target, mapper)),
                b: Box::new(b.map_leaf(target, mapper)),
            },
        }
    }

    /// Set the ratio of the split at `path` (`0` = first child).
    fn set_ratio(&self, path: &[usize], ratio: f32) -> LayoutNode {
        let Self::Split {
            direction,
            ratio: current,
            a,
            b,
        } = self
        else {
            return self.clone();
        };
        let next_ratio = if path.is_empty() {
            clamp_split_ratio(ratio, 0.0, MIN_DOCK_SIZE_PX)
        } else {
            *current
        };
        let (new_a, new_b) = match path.split_first() {
            None => (a.clone(), b.clone()),
            Some((0, rest)) => (Box::new(a.set_ratio(rest, ratio)), b.clone()),
            Some((_, rest)) => (a.clone(), Box::new(b.set_ratio(rest, ratio))),
        };
        Self::Split {
            direction: *direction,
            ratio: next_ratio,
            a: new_a,
            b: new_b,
        }
    }
}

/// Per-tab dock layout: one tree per edge plus the dock sizes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TabPluginLayout {
    pub left: Option<LayoutNode>,
    pub right: Option<LayoutNode>,
    pub top: Option<LayoutNode>,
    pub bottom: Option<LayoutNode>,
    pub overlay: Option<LayoutNode>,
    pub left_width_px: f32,
    pub right_width_px: f32,
    pub top_height_px: f32,
    pub bottom_height_px: f32,
}

impl Default for TabPluginLayout {
    fn default() -> Self {
        Self::empty()
    }
}

impl TabPluginLayout {
    /// No docks open.
    pub fn empty() -> Self {
        Self {
            left: None,
            right: None,
            top: None,
            bottom: None,
            overlay: None,
            left_width_px: DOCK_DEFAULT_WIDTH_PX,
            right_width_px: DOCK_DEFAULT_WIDTH_PX,
            top_height_px: DOCK_DEFAULT_HEIGHT_PX,
            bottom_height_px: DOCK_DEFAULT_HEIGHT_PX,
        }
    }

    /// The tree docked on `edge`.
    pub fn dock(&self, edge: DockEdge) -> Option<&LayoutNode> {
        match edge {
            DockEdge::Left => self.left.as_ref(),
            DockEdge::Right => self.right.as_ref(),
            DockEdge::Top => self.top.as_ref(),
            DockEdge::Bottom => self.bottom.as_ref(),
        }
    }

    /// Mutable access to the tree docked on `edge`.
    pub fn dock_mut(&mut self, edge: DockEdge) -> &mut Option<LayoutNode> {
        match edge {
            DockEdge::Left => &mut self.left,
            DockEdge::Right => &mut self.right,
            DockEdge::Top => &mut self.top,
            DockEdge::Bottom => &mut self.bottom,
        }
    }

    /// Configured size of a dock along its axis.
    pub fn size(&self, edge: DockEdge) -> f32 {
        match edge {
            DockEdge::Left => self.left_width_px,
            DockEdge::Right => self.right_width_px,
            DockEdge::Top => self.top_height_px,
            DockEdge::Bottom => self.bottom_height_px,
        }
    }

    /// Update a dock's size.
    pub fn set_size(&mut self, edge: DockEdge, px: f32) {
        let value = px.max(0.0);
        match edge {
            DockEdge::Left => self.left_width_px = value,
            DockEdge::Right => self.right_width_px = value,
            DockEdge::Top => self.top_height_px = value,
            DockEdge::Bottom => self.bottom_height_px = value,
        }
    }

    /// Every plugin id in the tab, in render order.
    pub fn plugin_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for edge in DockEdge::ALL {
            if let Some(node) = self.dock(edge) {
                ids.extend(node.plugin_ids());
            }
        }
        if let Some(node) = &self.overlay {
            ids.extend(node.plugin_ids());
        }
        ids
    }

    /// Whether `plugin_id` has a pane in this layout.
    pub fn contains(&self, plugin_id: &str) -> bool {
        self.plugin_ids().iter().any(|id| id == plugin_id)
    }

    /// True when no dock holds anything.
    pub fn is_empty(&self) -> bool {
        self.plugin_ids().is_empty()
    }

    /// Remove `plugin_id` from every edge.
    pub fn remove_plugin(&mut self, plugin_id: &str) {
        for edge in DockEdge::ALL {
            let slot = self.dock_mut(edge);
            if let Some(node) = slot.take() {
                *slot = node.remove(plugin_id);
            }
        }
        if let Some(node) = self.overlay.take() {
            self.overlay = node.remove(plugin_id);
        }
    }

    /// Dock `plugin_id` on `edge`.
    ///
    /// Inserting beside existing panes grows the dock by [`DOCK_GROW_STEP_PX`] so both
    /// stay usable.
    pub fn dock_on_edge(&mut self, plugin_id: &str, edge: DockEdge, zone: Option<LeafZone>) {
        self.remove_plugin(plugin_id);
        let had_content = self.dock(edge).is_some();
        let zone = zone.unwrap_or_else(|| edge.outer_zone());
        let incoming = LayoutNode::leaf(plugin_id);
        let slot = self.dock_mut(edge);
        *slot = Some(match slot.take() {
            None => incoming,
            Some(existing) => LayoutNode::wrap(existing, incoming, zone),
        });
        if had_content {
            let size = self.size(edge) + DOCK_GROW_STEP_PX;
            self.set_size(edge, size);
        }
    }

    /// Split the pane showing `target` and put `plugin_id` on `zone`.
    pub fn split_leaf(&mut self, target: &str, zone: LeafZone, plugin_id: &str) {
        if target == plugin_id {
            return;
        }
        self.remove_plugin(plugin_id);
        let incoming = plugin_id.to_owned();
        for edge in DockEdge::ALL {
            let Some(node) = self.dock_mut(edge) else {
                continue;
            };
            if !node.plugin_ids().iter().any(|id| id == target) {
                continue;
            }
            *node = node.map_leaf(target, &|_| {
                LayoutNode::wrap(
                    LayoutNode::leaf(target),
                    LayoutNode::leaf(incoming.clone()),
                    zone,
                )
            });
            return;
        }
        if let Some(node) = &self.overlay
            && node.plugin_ids().iter().any(|id| id == target)
        {
            self.overlay = Some(node.map_leaf(target, &|_| {
                LayoutNode::wrap(
                    LayoutNode::leaf(target),
                    LayoutNode::leaf(incoming.clone()),
                    zone,
                )
            }));
        }
    }

    /// Adjust the split ratio at `path` inside `edge`'s tree.
    pub fn set_split_ratio(&mut self, edge: DockEdge, path: &[usize], ratio: f32) {
        if let Some(node) = self.dock_mut(edge) {
            *node = node.set_ratio(path, ratio);
        }
    }

    /// Move `plugin_id` into the floating overlay.
    pub fn move_to_overlay(&mut self, plugin_id: &str) {
        self.remove_plugin(plugin_id);
        self.overlay = Some(LayoutNode::leaf(plugin_id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_strings_match_the_shared_contract() {
        assert_eq!(DockPlacement::SplitRight.as_str(), "split-right");
        assert_eq!(DockPlacement::SplitBottom.label(), "Bottom");
        assert!(DockPlacement::Overlay.is_overlay());
        assert!(!DockPlacement::SplitLeft.is_overlay());
    }

    #[test]
    fn dock_grows_to_requested_size_when_it_fits() {
        assert_eq!(clamp_dock_extent(300.0, 900.0), 300.0);
        assert_eq!(clamp_dock_extent(100.0, 900.0), MIN_DOCK_SIZE_PX);
    }

    #[test]
    fn dock_is_capped_so_the_terminal_survives() {
        assert_eq!(
            clamp_dock_extent(800.0, 900.0),
            900.0 - MIN_TERMINAL_WIDTH_PX - SPLITTER_SIZE_PX
        );
    }

    #[test]
    fn dock_collapses_when_there_is_no_room() {
        assert_eq!(clamp_dock_extent(300.0, 300.0), 0.0);
        assert_eq!(clamp_dock_extent(300.0, f32::NAN), 0.0);
        assert_eq!(clamp_dock_extent(300.0, 0.0), 0.0);
    }

    #[test]
    fn split_ratio_keeps_both_panes_usable() {
        let total = 1000.0;
        assert_eq!(clamp_split_ratio(0.5, total, 200.0), 0.5);
        assert_eq!(clamp_split_ratio(0.0, total, 200.0), 0.2);
        assert_eq!(clamp_split_ratio(1.0, total, 200.0), 0.8);
    }

    #[test]
    fn split_ratio_falls_back_for_tiny_containers() {
        let ratio = clamp_split_ratio(0.0, 300.0, 200.0);
        assert!((ratio - 0.01).abs() < f32::EPSILON);
        assert!((clamp_split_ratio(f32::NAN, 300.0, 200.0) - 0.5).abs() < f32::EPSILON);
    }

    // --- layout tree ------------------------------------------------------

    #[test]
    fn empty_layout_has_no_plugins() {
        let layout = TabPluginLayout::empty();
        assert!(layout.is_empty());
        assert!(layout.plugin_ids().is_empty());
        for edge in DockEdge::ALL {
            assert_eq!(layout.dock(edge), None);
            assert_eq!(
                layout.size(edge),
                if edge.is_vertical() {
                    DOCK_DEFAULT_WIDTH_PX
                } else {
                    DOCK_DEFAULT_HEIGHT_PX
                }
            );
        }
    }

    #[test]
    fn docking_the_first_plugin_keeps_the_default_size() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("sftp", DockEdge::Right, None);
        assert_eq!(layout.plugin_ids(), vec!["sftp".to_owned()]);
        assert_eq!(layout.size(DockEdge::Right), DOCK_DEFAULT_WIDTH_PX);
    }

    #[test]
    fn docking_beside_existing_content_grows_the_dock() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("sftp", DockEdge::Right, None);
        layout.dock_on_edge("docker-manager", DockEdge::Right, None);
        assert_eq!(
            layout.plugin_ids(),
            vec!["sftp".to_owned(), "docker-manager".to_owned()],
            "outer insert puts the new pane last"
        );
        assert_eq!(
            layout.size(DockEdge::Right),
            DOCK_DEFAULT_WIDTH_PX + DOCK_GROW_STEP_PX
        );
    }

    #[test]
    fn inner_zone_puts_the_new_pane_closest_to_the_terminal() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("sftp", DockEdge::Right, None);
        layout.dock_on_edge(
            "server-monitor",
            DockEdge::Right,
            Some(DockEdge::Right.inner_zone()),
        );
        assert_eq!(
            layout.plugin_ids(),
            vec!["server-monitor".to_owned(), "sftp".to_owned()]
        );
    }

    #[test]
    fn a_plugin_moves_rather_than_duplicating() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("sftp", DockEdge::Right, None);
        layout.dock_on_edge("sftp", DockEdge::Left, None);
        assert!(layout.right.is_none());
        assert_eq!(layout.plugin_ids(), vec!["sftp".to_owned()]);
    }

    #[test]
    fn removing_a_pane_collapses_its_split() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("a", DockEdge::Left, None);
        layout.dock_on_edge("b", DockEdge::Left, None);
        assert!(matches!(layout.left, Some(LayoutNode::Split { .. })));
        layout.remove_plugin("b");
        assert_eq!(layout.plugin_ids(), vec!["a".to_owned()]);
        assert!(matches!(layout.left, Some(LayoutNode::Leaf { .. })));
        layout.remove_plugin("a");
        assert!(layout.left.is_none());
        assert!(layout.is_empty());
    }

    #[test]
    fn splitting_a_leaf_wraps_it_in_a_row_or_column() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("sftp", DockEdge::Right, None);
        layout.split_leaf("sftp", LeafZone::Top, "scratchpad");
        let node = layout.right.as_ref().expect("right dock");
        match node {
            LayoutNode::Split {
                direction, ratio, a, b,
            } => {
                assert_eq!(*direction, SplitDirection::Column);
                assert!((ratio - 0.5).abs() < f32::EPSILON);
                assert_eq!(a.plugin_ids(), vec!["scratchpad".to_owned()]);
                assert_eq!(b.plugin_ids(), vec!["sftp".to_owned()]);
            }
            other => panic!("expected a split, got {other:?}"),
        }
    }

    #[test]
    fn overlay_holds_its_own_tree() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("sftp", DockEdge::Right, None);
        layout.move_to_overlay("sftp");
        assert!(layout.right.is_none());
        assert!(layout.contains("sftp"));
        assert_eq!(layout.overlay, Some(LayoutNode::leaf("sftp")));
    }

    #[test]
    fn split_ratios_are_clamped() {
        let mut layout = TabPluginLayout::empty();
        layout.dock_on_edge("a", DockEdge::Left, None);
        layout.dock_on_edge("b", DockEdge::Left, None);
        layout.set_split_ratio(DockEdge::Left, &[], 5.0);
        match layout.left.as_ref().unwrap() {
            LayoutNode::Split { ratio, .. } => {
                assert!(*ratio <= 0.99 && *ratio >= 0.01, "ratio was {ratio}");
            }
            other => panic!("expected a split, got {other:?}"),
        }
    }

    #[test]
    fn layout_json_matches_the_electron_shape() {
        let layout = TabPluginLayout {
            right: Some(LayoutNode::Split {
                direction: SplitDirection::Column,
                ratio: 0.5,
                a: Box::new(LayoutNode::leaf("sftp")),
                b: Box::new(LayoutNode::leaf("scratchpad")),
            }),
            ..TabPluginLayout::empty()
        };
        let json = serde_json::to_value(&layout).expect("serialise");
        assert_eq!(json["right"]["kind"], "split");
        assert_eq!(json["right"]["direction"], "column");
        assert_eq!(json["right"]["a"]["pluginId"], "sftp");
        assert_eq!(json["rightWidthPx"], serde_json::json!(300.0));

        let parsed: TabPluginLayout = serde_json::from_value(json).expect("deserialise");
        assert_eq!(parsed, layout);
    }

    #[test]
    fn partial_layout_json_falls_back_to_defaults() {
        let parsed: TabPluginLayout =
            serde_json::from_str(r#"{"left":{"kind":"leaf","pluginId":"sftp"}}"#)
                .expect("deserialise");
        assert!(parsed.contains("sftp"));
        assert_eq!(parsed.left_width_px, DOCK_DEFAULT_WIDTH_PX);
        assert_eq!(parsed.bottom_height_px, DOCK_DEFAULT_HEIGHT_PX);
    }
}
