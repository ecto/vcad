//! TUI mode definitions and transitions.

#![allow(dead_code)] // Modes will be used as TUI features are implemented

use vcad_ir::NodeId;
use vcad_kernel_constraints::{SketchPlane, SketchSession, SketchTool};

/// Active TUI editing mode.
#[derive(Debug, Clone, Default)]
pub enum TuiMode {
    /// Normal 3D editing mode - primitives, booleans, transforms
    #[default]
    Normal,
    /// Command input mode (: or / pressed)
    Command,
    /// Sketch mode - 2D constraint-based drawing
    ///
    /// Boxed because [`SketchModeState`] owns a full kernel session with
    /// undo/redo history snapshots, which is much larger than the other
    /// mode payloads.
    Sketch(Box<SketchModeState>),
    /// Assembly mode - instances, joints, forward kinematics
    Assembly(AssemblyModeState),
    /// Physics simulation mode
    Physics(PhysicsModeState),
    /// CAM mode - toolpath generation
    Cam(CamModeState),
    /// 3D print mode - slicing and printer control
    Print(PrintModeState),
}

impl TuiMode {
    /// Get the mode name for display.
    pub fn name(&self) -> &'static str {
        match self {
            TuiMode::Normal => "NORMAL",
            TuiMode::Command => "COMMAND",
            TuiMode::Sketch(_) => "SKETCH",
            TuiMode::Assembly(_) => "ASSEMBLY",
            TuiMode::Physics(_) => "PHYSICS",
            TuiMode::Cam(_) => "CAM",
            TuiMode::Print(_) => "PRINT",
        }
    }

    /// Get hotkey hints for the current mode.
    pub fn hotkey_hints(&self) -> &'static str {
        match self {
            TuiMode::Normal => "1-3:prim  u/r:undo  wasd:move  Tab:select  ::cmd  q:quit",
            TuiMode::Command => "Enter:exec  Esc:cancel",
            TuiMode::Sketch(_) => "l:line  r:rect  c:circ  h:horiz  v:vert  x:extrude  Esc:exit",
            TuiMode::Assembly(_) => "i:instance  j:joint  LeftRight:adjust  k:FK  Esc:exit",
            TuiMode::Physics(_) => "Space:play  .:step  r:reset  1-9:torque  Esc:exit",
            TuiMode::Cam(_) => "f:face  p:pocket  g:generate  x:export  Esc:exit",
            TuiMode::Print(_) => "s:slice  g:gcode  c:connect  x:send  Esc:exit",
        }
    }

    /// Check if this mode allows viewport navigation.
    pub fn allows_viewport_nav(&self) -> bool {
        // All modes except command allow camera movement
        !matches!(self, TuiMode::Command)
    }

    /// Check if this mode is a sub-mode (not Normal or Command).
    pub fn is_submode(&self) -> bool {
        matches!(
            self,
            TuiMode::Sketch(_)
                | TuiMode::Assembly(_)
                | TuiMode::Physics(_)
                | TuiMode::Cam(_)
                | TuiMode::Print(_)
        )
    }
}

/// Sketch mode state, backed by the kernel [`SketchSession`].
///
/// This is a thin shell around the kernel session — the TUI and the web app
/// share the same session implementation via `vcad-kernel-constraints::session`,
/// so any tool-state or solver improvement shows up in both frontends.
#[derive(Debug, Clone)]
pub struct SketchModeState {
    /// The kernel-backed editing session.
    pub session: SketchSession,
    /// Target face for the sketch, if the plane came from a face pick.
    pub target_face: Option<NodeId>,
}

impl Default for SketchModeState {
    fn default() -> Self {
        Self {
            session: SketchSession::new(SketchPlane::XY),
            target_face: None,
        }
    }
}

impl SketchModeState {
    /// Create a new sketch mode on the given plane.
    pub fn new(plane: SketchPlane) -> Self {
        Self {
            session: SketchSession::new(plane),
            target_face: None,
        }
    }

    /// Current tool.
    pub fn tool(&self) -> SketchTool {
        self.session.tool()
    }

    /// Current cursor position in sketch coordinates (if any).
    pub fn cursor(&self) -> [f64; 2] {
        self.session
            .cursor()
            .map(|c| [c.x, c.y])
            .unwrap_or([0.0, 0.0])
    }
}

/// Assembly mode state.
#[derive(Debug, Clone, Default)]
pub struct AssemblyModeState {
    /// Currently selected instance index
    pub selected_instance: Option<usize>,
    /// Currently selected joint index
    pub selected_joint: Option<usize>,
    /// Forward kinematics enabled
    pub fk_enabled: bool,
    /// Joint positions (joint name -> position)
    pub joint_positions: std::collections::HashMap<String, f64>,
    /// UI focus area
    pub focus: AssemblyFocus,
}

/// Assembly mode UI focus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AssemblyFocus {
    #[default]
    PartList,
    InstanceList,
    JointList,
    Properties,
}

/// Physics simulation mode state.
#[derive(Debug, Clone, Default)]
pub struct PhysicsModeState {
    /// Simulation running
    pub running: bool,
    /// Simulation time
    pub time: f64,
    /// Time step
    pub dt: f64,
    /// Selected joint for control
    pub selected_joint: Option<usize>,
    /// Applied actions (joint name -> action)
    pub actions: std::collections::HashMap<String, PhysicsAction>,
    /// Recording trajectory
    pub recording: bool,
}

/// Physics action type.
#[derive(Debug, Clone, Copy)]
pub enum PhysicsAction {
    /// Apply torque (Nm)
    Torque(f64),
    /// Target position (rad or m)
    PositionTarget(f64),
    /// Target velocity (rad/s or m/s)
    VelocityTarget(f64),
}

/// CAM mode state.
#[derive(Debug, Clone, Default)]
pub struct CamModeState {
    /// Selected tool index
    pub selected_tool: usize,
    /// Selected operation index
    pub selected_op: Option<usize>,
    /// Toolpath generated
    pub has_toolpath: bool,
    /// G-code generated
    pub has_gcode: bool,
    /// UI focus area
    pub focus: CamFocus,
}

/// CAM mode UI focus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CamFocus {
    #[default]
    ToolList,
    OperationList,
    Settings,
    Preview,
}

/// 3D print mode state.
#[derive(Debug, Clone, Default)]
pub struct PrintModeState {
    /// Slicing completed
    pub sliced: bool,
    /// G-code generated
    pub has_gcode: bool,
    /// Current preview layer
    pub preview_layer: usize,
    /// Total layers
    pub total_layers: usize,
    /// Connected printer address
    pub printer_address: Option<String>,
    /// Printer connection status
    pub printer_connected: bool,
    /// UI focus area
    pub focus: PrintFocus,
}

/// Print mode UI focus.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrintFocus {
    #[default]
    Settings,
    Preview,
    Printer,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mode_names() {
        assert_eq!(TuiMode::Normal.name(), "NORMAL");
        assert_eq!(TuiMode::Sketch(Box::default()).name(), "SKETCH");
    }

    #[test]
    fn test_submode_detection() {
        assert!(!TuiMode::Normal.is_submode());
        assert!(!TuiMode::Command.is_submode());
        assert!(TuiMode::Sketch(Box::<SketchModeState>::default()).is_submode());
        assert!(TuiMode::Physics(Default::default()).is_submode());
    }
}
