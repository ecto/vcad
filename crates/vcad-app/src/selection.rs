//! Selection state management.
//!
//! Tagged-union storage that mirrors the TS `SelectionItem` shape. The
//! existing part-only API (`select` / `toggle` / `contains` / `single` /
//! `ids`) is preserved for back-compat — internally these operate on items
//! of `SelectionItem::Part(_)`. Sub-feature kinds (face / edge / vertex)
//! are interactive only; they're dropped when topology changes.

use std::collections::HashSet;
use vcad_ir::NodeId;

/// Selection filter — restricts what the picker will return on the next
/// pointer event. Mirrors the TS `SelectionFilter`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectionFilter {
    #[default]
    Auto,
    Body,
    Face,
    Edge,
    Vertex,
}

/// One thing the user can hover or click on. Mirrors the TS `SelectionItem`
/// discriminated union.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SelectionItem {
    Part(NodeId),
    Face { part_id: NodeId, face_index: u32 },
    Edge { part_id: NodeId, edge_id: u32 },
    Vertex { part_id: NodeId, vertex_id: u32 },
    Segment { sketch_id: String, index: u32 },
    Constraint { sketch_id: String, index: u32 },
}

impl SelectionItem {
    /// If this item is associated with a part, return its `NodeId`.
    pub fn part_id(&self) -> Option<NodeId> {
        match self {
            SelectionItem::Part(id) => Some(*id),
            SelectionItem::Face { part_id, .. } => Some(*part_id),
            SelectionItem::Edge { part_id, .. } => Some(*part_id),
            SelectionItem::Vertex { part_id, .. } => Some(*part_id),
            SelectionItem::Segment { .. } | SelectionItem::Constraint { .. } => None,
        }
    }
}

/// Manages the current selection.
///
/// Stores items as a `Vec` so order is preserved (matches the TS array shape
/// the web UI uses for ordered display). Lookup helpers (`contains`, `ids`,
/// `single`) walk the vec — selection is small (typically < 10 items), so
/// the linear scan is fine.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    items: Vec<SelectionItem>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a single part (replacing current selection).
    pub fn select(&mut self, id: NodeId) {
        self.items.clear();
        self.items.push(SelectionItem::Part(id));
    }

    /// Add a part to the selection.
    pub fn select_add(&mut self, id: NodeId) {
        if !self.items.iter().any(|it| matches!(it, SelectionItem::Part(p) if *p == id)) {
            self.items.push(SelectionItem::Part(id));
        }
    }

    /// Toggle a part's selection state.
    pub fn toggle(&mut self, id: NodeId) {
        let pos = self
            .items
            .iter()
            .position(|it| matches!(it, SelectionItem::Part(p) if *p == id));
        match pos {
            Some(i) => {
                self.items.remove(i);
            }
            None => self.items.push(SelectionItem::Part(id)),
        }
    }

    /// Deselect a specific part.
    pub fn deselect(&mut self, id: NodeId) {
        self.items
            .retain(|it| !matches!(it, SelectionItem::Part(p) if *p == id));
    }

    /// Clear the entire selection.
    pub fn clear(&mut self) {
        self.items.clear();
    }

    /// Check if a part is selected.
    pub fn contains(&self, id: NodeId) -> bool {
        self.items
            .iter()
            .any(|it| matches!(it, SelectionItem::Part(p) if *p == id))
    }

    /// Check if selection is empty.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of selected items.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Number of selected parts (excluding sub-features and sketch entities).
    pub fn part_count(&self) -> usize {
        self.items
            .iter()
            .filter(|it| matches!(it, SelectionItem::Part(_)))
            .count()
    }

    /// Get the selected part IDs as a set. Derived view — Vec → HashSet
    /// each call, but selection is tiny so the cost is negligible.
    pub fn ids(&self) -> HashSet<NodeId> {
        self.items
            .iter()
            .filter_map(|it| match it {
                SelectionItem::Part(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    /// Get a single selected part ID (if exactly one part is selected,
    /// ignoring any sub-feature items).
    pub fn single(&self) -> Option<NodeId> {
        let mut iter = self.items.iter().filter_map(|it| match it {
            SelectionItem::Part(id) => Some(*id),
            _ => None,
        });
        let first = iter.next()?;
        if iter.next().is_some() {
            None
        } else {
            Some(first)
        }
    }

    /// Select multiple parts at once (replacing current selection).
    pub fn select_many(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        self.items.clear();
        for id in ids {
            self.items.push(SelectionItem::Part(id));
        }
    }

    /// Remove parts that no longer exist in the document. Sub-feature
    /// items whose owning part is gone are also removed.
    pub fn retain(&mut self, valid: &HashSet<NodeId>) {
        self.items.retain(|it| match it {
            SelectionItem::Part(id) => valid.contains(id),
            SelectionItem::Face { part_id, .. }
            | SelectionItem::Edge { part_id, .. }
            | SelectionItem::Vertex { part_id, .. } => valid.contains(part_id),
            SelectionItem::Segment { .. } | SelectionItem::Constraint { .. } => true,
        });
    }

    // ── Sub-feature selection ──────────────────────────────────────────

    /// Replace the entire selection with a single tagged item.
    pub fn select_item(&mut self, item: SelectionItem) {
        self.items.clear();
        self.items.push(item);
    }

    /// Toggle a tagged item in/out of the selection.
    pub fn toggle_item(&mut self, item: SelectionItem) {
        match self.items.iter().position(|existing| existing == &item) {
            Some(i) => {
                self.items.remove(i);
            }
            None => self.items.push(item),
        }
    }

    /// Replace the entire selection with a list of items.
    pub fn select_items(&mut self, items: impl IntoIterator<Item = SelectionItem>) {
        self.items = items.into_iter().collect();
    }

    /// Borrow the full item list.
    pub fn items(&self) -> &[SelectionItem] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_select_replace() {
        let mut sel = Selection::new();
        sel.select(1);
        sel.select(2);
        assert_eq!(sel.len(), 1);
        assert!(sel.contains(2));
        assert!(!sel.contains(1));
    }

    #[test]
    fn test_select_add() {
        let mut sel = Selection::new();
        sel.select(1);
        sel.select_add(2);
        assert_eq!(sel.len(), 2);
    }

    #[test]
    fn test_toggle() {
        let mut sel = Selection::new();
        sel.toggle(1);
        assert!(sel.contains(1));
        sel.toggle(1);
        assert!(!sel.contains(1));
    }

    #[test]
    fn test_single() {
        let mut sel = Selection::new();
        assert!(sel.single().is_none());
        sel.select(42);
        assert_eq!(sel.single(), Some(42));
        sel.select_add(43);
        assert!(sel.single().is_none());
    }

    #[test]
    fn test_ids_returns_only_parts() {
        let mut sel = Selection::new();
        sel.select(1);
        sel.toggle_item(SelectionItem::Face {
            part_id: 1,
            face_index: 3,
        });
        sel.toggle_item(SelectionItem::Edge {
            part_id: 2,
            edge_id: 7,
        });
        let ids = sel.ids();
        assert_eq!(ids.len(), 1);
        assert!(ids.contains(&1));
        // Edge added 2 to selection but not as a Part — `ids()` filters it out.
        assert!(!ids.contains(&2));
    }

    #[test]
    fn test_part_count_excludes_sub_features() {
        let mut sel = Selection::new();
        sel.select(1);
        sel.toggle_item(SelectionItem::Face {
            part_id: 1,
            face_index: 0,
        });
        assert_eq!(sel.len(), 2);
        assert_eq!(sel.part_count(), 1);
    }

    #[test]
    fn test_select_item_replaces_everything() {
        let mut sel = Selection::new();
        sel.select(1);
        sel.select_add(2);
        sel.select_item(SelectionItem::Face {
            part_id: 9,
            face_index: 4,
        });
        assert_eq!(sel.len(), 1);
        assert!(matches!(
            sel.items()[0],
            SelectionItem::Face { face_index: 4, .. }
        ));
    }

    #[test]
    fn test_toggle_item_round_trip() {
        let mut sel = Selection::new();
        let face = SelectionItem::Face {
            part_id: 1,
            face_index: 2,
        };
        sel.toggle_item(face.clone());
        assert!(sel.items().contains(&face));
        sel.toggle_item(face.clone());
        assert!(!sel.items().contains(&face));
    }

    #[test]
    fn test_retain_drops_sub_features_for_missing_parts() {
        let mut sel = Selection::new();
        sel.select(1);
        sel.toggle_item(SelectionItem::Face {
            part_id: 2,
            face_index: 0,
        });
        let valid: HashSet<NodeId> = [1].into_iter().collect();
        sel.retain(&valid);
        assert_eq!(sel.len(), 1);
        assert!(sel.contains(1));
    }
}
