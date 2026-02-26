//! DocumentEngine — unified document state and operations.

use anyhow::Result;
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3};

use crate::camera::Camera;
use crate::evaluate::{evaluate_document, EvaluatedScene};
use crate::selection::Selection;

/// Central application engine owning all document state.
pub struct DocumentEngine {
    document: Document,
    pub selection: Selection,
    pub camera: Camera,
    undo_stack: Vec<Document>,
    redo_stack: Vec<Document>,
    next_node_id: NodeId,
    dirty: bool,
    scene: EvaluatedScene,
}

impl DocumentEngine {
    /// Create a new engine with an empty document.
    pub fn new() -> Self {
        let doc = Document::new();
        let scene =
            evaluate_document(&doc).unwrap_or_else(|_| EvaluatedScene { meshes: Vec::new() });
        Self {
            document: doc,
            selection: Selection::new(),
            camera: Camera::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_node_id: 1,
            dirty: false,
            scene,
        }
    }

    /// Create an engine from an existing document.
    pub fn from_document(doc: Document) -> Result<Self> {
        let next_id = doc.nodes.keys().copied().max().unwrap_or(0) + 1;
        let scene = evaluate_document(&doc)?;
        Ok(Self {
            document: doc,
            selection: Selection::new(),
            camera: Camera::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_node_id: next_id,
            dirty: false,
            scene,
        })
    }

    /// Load from JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        let doc = Document::from_json(json)?;
        Self::from_document(doc)
    }

    /// Serialize to JSON.
    pub fn to_json(&self) -> Result<String> {
        Ok(self.document.to_json()?)
    }

    /// Get a reference to the document.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// Get a mutable reference to the document (use sparingly).
    pub fn document_mut(&mut self) -> &mut Document {
        &mut self.document
    }

    /// Get the current evaluated scene.
    pub fn scene(&self) -> &EvaluatedScene {
        &self.scene
    }

    /// Whether the document has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Mark as saved.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    // -- Undo/Redo --

    fn push_undo(&mut self) {
        self.undo_stack.push(self.document.clone());
        self.redo_stack.clear();
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
        self.dirty = true;
    }

    pub fn undo(&mut self) -> Result<bool> {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.document.clone());
            self.document = prev;
            self.re_evaluate()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn redo(&mut self) -> Result<bool> {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.document.clone());
            self.document = next;
            self.re_evaluate()?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    // -- Node allocation --

    fn alloc_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    // -- Mutations --

    /// Add a primitive and return its NodeId.
    pub fn add_primitive(&mut self, op: CsgOp, name: Option<String>) -> Result<NodeId> {
        self.push_undo();
        let id = self.alloc_node_id();
        self.document.nodes.insert(
            id,
            Node {
                id,
                name: name.or_else(|| Some(format!("Part {}", id))),
                op,
            },
        );
        self.document.roots.push(SceneEntry {
            root: id,
            material: "default".to_string(),
            visible: None,
        });
        self.re_evaluate()?;
        Ok(id)
    }

    /// Add a cube.
    pub fn add_cube(&mut self, size: f64) -> Result<NodeId> {
        self.add_primitive(
            CsgOp::Cube {
                size: Vec3::new(size, size, size),
            },
            Some(format!("Cube {}", self.next_node_id)),
        )
    }

    /// Add a cylinder.
    pub fn add_cylinder(&mut self, radius: f64, height: f64) -> Result<NodeId> {
        self.add_primitive(
            CsgOp::Cylinder {
                radius,
                height,
                segments: 32,
            },
            Some(format!("Cylinder {}", self.next_node_id)),
        )
    }

    /// Add a sphere.
    pub fn add_sphere(&mut self, radius: f64) -> Result<NodeId> {
        self.add_primitive(
            CsgOp::Sphere {
                radius,
                segments: 32,
            },
            Some(format!("Sphere {}", self.next_node_id)),
        )
    }

    /// Delete selected nodes.
    pub fn delete_selected(&mut self) -> Result<usize> {
        let ids = self.selection.ids().clone();
        if ids.is_empty() {
            return Ok(0);
        }
        self.push_undo();
        self.document.roots.retain(|e| !ids.contains(&e.root));
        for id in &ids {
            self.document.nodes.remove(id);
        }
        let count = ids.len();
        self.selection.clear();
        self.re_evaluate()?;
        Ok(count)
    }

    /// Translate selected nodes.
    pub fn translate_selected(&mut self, dx: f64, dy: f64, dz: f64) -> Result<()> {
        let selected_ids: Vec<NodeId> = self.selection.ids().iter().copied().collect();
        if selected_ids.is_empty() {
            return Ok(());
        }
        self.push_undo();

        for selected_id in &selected_ids {
            if let Some(idx) = self
                .document
                .roots
                .iter()
                .position(|e| e.root == *selected_id)
            {
                let old_root = self.document.roots[idx].root;
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: self
                            .document
                            .nodes
                            .get(&old_root)
                            .and_then(|n| n.name.clone()),
                        op: CsgOp::Translate {
                            child: old_root,
                            offset: Vec3::new(dx, dy, dz),
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selection.deselect(*selected_id);
                self.selection.select_add(new_id);
            }
        }

        self.re_evaluate()?;
        Ok(())
    }

    /// Get the list of parts for the tree view.
    pub fn get_parts(&self) -> Vec<(NodeId, String)> {
        self.document
            .roots
            .iter()
            .map(|e| {
                let name = self
                    .document
                    .nodes
                    .get(&e.root)
                    .and_then(|n| n.name.clone())
                    .unwrap_or_else(|| format!("Node {}", e.root));
                (e.root, name)
            })
            .collect()
    }

    /// Re-evaluate the document after a mutation.
    pub fn re_evaluate(&mut self) -> Result<()> {
        self.scene = evaluate_document(&self.document)?;
        Ok(())
    }
}

impl Default for DocumentEngine {
    fn default() -> Self {
        Self::new()
    }
}
