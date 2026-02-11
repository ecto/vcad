//! TUI mode definitions and transitions.

#![allow(dead_code)] // Modes will be used as TUI features are implemented

use vcad_ir::NodeId;

/// Active TUI editing mode.
#[derive(Debug, Clone, Default)]
pub enum TuiMode {
    /// Normal 3D editing mode - primitives, booleans, transforms
    #[default]
    Normal,
    /// Command input mode (: or / pressed)
    Command,
    /// Sketch mode - 2D constraint-based drawing
    Sketch(SketchModeState),
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

/// Sketch mode state.
#[derive(Debug, Clone, Default)]
pub struct SketchModeState {
    /// Current drawing tool
    pub tool: SketchTool,
    /// Sketch plane
    pub plane: SketchPlane,
    /// Selected entity indices
    pub selected_entities: Vec<usize>,
    /// Cursor position in sketch coordinates
    pub cursor: [f64; 2],
    /// Pending line start point (if drawing)
    pub pending_start: Option<[f64; 2]>,
    /// Target face for sketch (if any)
    pub target_face: Option<NodeId>,
}

/// Sketch drawing tool.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SketchTool {
    #[default]
    Select,
    Line,
    Rectangle,
    Circle,
    Arc,
    Point,
}

/// Sketch plane orientation.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum SketchPlane {
    #[default]
    XY,
    XZ,
    YZ,
    Custom {
        origin: [f64; 3],
        normal: [f64; 3],
    },
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
        assert_eq!(TuiMode::Sketch(Default::default()).name(), "SKETCH");
    }

    #[test]
    fn test_submode_detection() {
        assert!(!TuiMode::Normal.is_submode());
        assert!(!TuiMode::Command.is_submode());
        assert!(TuiMode::Sketch(Default::default()).is_submode());
        assert!(TuiMode::Physics(Default::default()).is_submode());
    }
}
