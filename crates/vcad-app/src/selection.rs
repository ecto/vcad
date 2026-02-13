//! Selection state management.

use std::collections::HashSet;
use vcad_ir::NodeId;

/// Manages the current selection of nodes.
#[derive(Debug, Clone, Default)]
pub struct Selection {
    selected: HashSet<NodeId>,
}

impl Selection {
    pub fn new() -> Self {
        Self::default()
    }

    /// Select a single node (replacing current selection).
    pub fn select(&mut self, id: NodeId) {
        self.selected.clear();
        self.selected.insert(id);
    }

    /// Add a node to the selection.
    pub fn select_add(&mut self, id: NodeId) {
        self.selected.insert(id);
    }

    /// Toggle a node's selection state.
    pub fn toggle(&mut self, id: NodeId) {
        if !self.selected.remove(&id) {
            self.selected.insert(id);
        }
    }

    /// Deselect a specific node.
    pub fn deselect(&mut self, id: NodeId) {
        self.selected.remove(&id);
    }

    /// Clear the entire selection.
    pub fn clear(&mut self) {
        self.selected.clear();
    }

    /// Check if a node is selected.
    pub fn contains(&self, id: NodeId) -> bool {
        self.selected.contains(&id)
    }

    /// Check if selection is empty.
    pub fn is_empty(&self) -> bool {
        self.selected.is_empty()
    }

    /// Number of selected nodes.
    pub fn len(&self) -> usize {
        self.selected.len()
    }

    /// Get the selected node IDs.
    pub fn ids(&self) -> &HashSet<NodeId> {
        &self.selected
    }

    /// Get a single selected ID (if exactly one is selected).
    pub fn single(&self) -> Option<NodeId> {
        if self.selected.len() == 1 {
            self.selected.iter().copied().next()
        } else {
            None
        }
    }

    /// Select multiple nodes at once (replacing current selection).
    pub fn select_many(&mut self, ids: impl IntoIterator<Item = NodeId>) {
        self.selected.clear();
        self.selected.extend(ids);
    }

    /// Remove nodes that no longer exist in the document.
    pub fn retain(&mut self, valid: &HashSet<NodeId>) {
        self.selected.retain(|id| valid.contains(id));
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
}
