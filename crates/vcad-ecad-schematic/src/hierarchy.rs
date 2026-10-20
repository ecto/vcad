//! Hierarchical sheets: flatten a [`SchematicSheet`] carrying a
//! [`SchematicHierarchy`] into one flat sheet (components + explicit nets).
//!
//! Scoping rules (mirrored by `packages/mcp/src/tools/schematic-hierarchy.ts`):
//! - A net name inside a child sheet is **local** to the sheet instance and
//!   flattens to `<path>/<name>`, where the path is the instance names from
//!   the root joined by `/` (`CAN1/CANH`).
//! - A name that is a **port** of the sheet (declared in `ports`, or a
//!   Hierarchical-scope label) resolves through the instance's
//!   `connections` to a net in the parent's scope.
//! - A name in `global_nets`, or carried by a Global-scope label anywhere, is
//!   **global** and keeps its name in every sheet.
//! - Root-sheet names are flat names as-is.
//!
//! Reference designators: an instance's `ref_map` wins, then `ref_offset`
//! (`U1` + 100 → `U101`); otherwise a sheet used once keeps its refs and a
//! repeated sheet gets `<ref>_<instance>`.
//!
//! Structural problems (unknown sheet, cycles, unbound/unknown ports, scope
//! collisions) are returned as ERC violations, not errors, so a partly broken
//! design still flattens and the rest of ERC still runs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use vcad_ir::ecad::*;
use vcad_ir::Vec2;

use crate::erc::{ErcSeverity, ErcViolation};
use crate::generate_netlist;

/// One elaborated sheet instance.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FlatInstance {
    /// Instance path from the root (`"CAN1"`, `"PWR/BUCK2"`).
    pub path: String,
    /// Sheet definition name.
    pub sheet: String,
    /// Local ref → flattened ref.
    pub refs: BTreeMap<String, String>,
}

/// Result of [`flatten`].
#[derive(Debug, Clone, PartialEq)]
pub struct Flattened {
    /// The flat sheet (`hierarchy` is always `None`).
    pub sheet: SchematicSheet,
    /// Hierarchy-level ERC findings.
    pub diagnostics: Vec<ErcViolation>,
    /// Every elaborated instance, in elaboration order.
    pub instances: Vec<FlatInstance>,
}

/// Vertical spacing between auto-placed instances in the flat sheet.
pub const AUTO_INSTANCE_STRIDE: f64 = 100_000.0;

fn diag(severity: ErcSeverity, message: String) -> ErcViolation {
    ErcViolation {
        severity,
        message,
        position: None,
    }
}

/// Renumber a reference designator: `U1` + 100 → `U101`. A ref with no
/// trailing digits gets the offset appended (`J` + 100 → `J100`).
pub fn offset_ref(reference: &str, offset: u32) -> String {
    let digits_at = reference
        .rfind(|c: char| !c.is_ascii_digit())
        .map(|i| i + 1)
        .unwrap_or(0);
    let (prefix, digits) = reference.split_at(digits_at);
    match digits.parse::<u64>() {
        Ok(n) => format!("{prefix}{}", n + u64::from(offset)),
        Err(_) => format!("{reference}{offset}"),
    }
}

/// Is `name` an auto-generated (anonymous) netlist name?
fn is_auto_name(name: &str) -> bool {
    name.strip_prefix("NET-")
        .is_some_and(|d| !d.is_empty() && d.chars().all(|c| c.is_ascii_digit()))
}

struct Ctx<'a> {
    defs: HashMap<&'a str, &'a SheetDefinition>,
    globals: BTreeSet<String>,
    root_names: BTreeSet<String>,
    use_count: HashMap<String, usize>,
    unused_port_reported: BTreeSet<String>,
    auto_slot: usize,
    out_components: Vec<SchematicComponent>,
    out_nets: BTreeMap<String, Vec<String>>,
    out_flags: Vec<PowerFlag>,
    instances: Vec<FlatInstance>,
    diagnostics: Vec<ErcViolation>,
}

fn ports_of(def: &SheetDefinition) -> BTreeSet<String> {
    let mut ports: BTreeSet<String> = def.ports.iter().map(|p| p.name.clone()).collect();
    for l in &def.body.labels {
        if l.scope == LabelScope::Hierarchical {
            ports.insert(l.name.clone());
        }
    }
    ports
}

fn count_uses<'a>(
    defs: &HashMap<&'a str, &'a SheetDefinition>,
    instances: &'a [SheetInstance],
    counts: &mut HashMap<String, usize>,
    stack: &mut Vec<&'a str>,
) {
    for inst in instances {
        *counts.entry(inst.sheet.clone()).or_default() += 1;
        if stack.contains(&inst.sheet.as_str()) {
            continue;
        }
        if let Some(def) = defs.get(inst.sheet.as_str()) {
            stack.push(&def.name);
            count_uses(defs, &def.instances, counts, stack);
            stack.pop();
        }
    }
}

impl<'a> Ctx<'a> {
    /// Elaborate every instance in `instances`, whose parent is at
    /// `parent_path` with name resolver `resolve_parent`.
    fn elaborate_all(
        &mut self,
        instances: &'a [SheetInstance],
        parent_path: &str,
        parent_offset: Vec2,
        resolve_parent: &dyn Fn(&str) -> String,
        stack: &mut Vec<&'a str>,
    ) {
        let mut seen = BTreeSet::new();
        for inst in instances {
            let where_ = if parent_path.is_empty() {
                "the root sheet".to_string()
            } else {
                format!("sheet instance '{parent_path}'")
            };
            if inst.name.is_empty() || inst.name.contains('/') {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!(
                        "Sheet instance name '{}' in {where_} must be non-empty and contain no '/'",
                        inst.name
                    ),
                ));
                continue;
            }
            if !seen.insert(inst.name.as_str()) {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!("Duplicate sheet instance name '{}' in {where_}", inst.name),
                ));
                continue;
            }
            let Some(def) = self.defs.get(inst.sheet.as_str()).copied() else {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!(
                        "Sheet instance '{}' references unknown sheet '{}'",
                        inst.name, inst.sheet
                    ),
                ));
                continue;
            };
            if stack.contains(&def.name.as_str()) {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!(
                        "Sheet recursion: instance '{}' places sheet '{}' inside itself",
                        inst.name, def.name
                    ),
                ));
                continue;
            }
            let path = if parent_path.is_empty() {
                inst.name.clone()
            } else {
                format!("{parent_path}/{}", inst.name)
            };
            let binding: BTreeMap<String, String> = inst
                .connections
                .iter()
                .map(|(port, net)| (port.clone(), resolve_parent(net)))
                .collect();
            // Instances without a position are moved to their own band of the flat
            // sheet, so pins of different instances (drawn at the same local
            // coordinates) never meet by coordinate coincidence.
            self.auto_slot += 1;
            let offset = match inst.position {
                Some(p) => Vec2::new(parent_offset.x + p.x, parent_offset.y + p.y),
                None => Vec2::new(0.0, AUTO_INSTANCE_STRIDE * self.auto_slot as f64),
            };
            stack.push(&def.name);
            self.elaborate(def, inst, &path, &binding, offset, stack);
            stack.pop();
        }
    }

    fn elaborate(
        &mut self,
        def: &'a SheetDefinition,
        inst: &'a SheetInstance,
        path: &str,
        binding: &BTreeMap<String, String>,
        offset: Vec2,
        stack: &mut Vec<&'a str>,
    ) {
        let ports = ports_of(def);

        for port in &ports {
            if self.globals.contains(port) {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!(
                        "Net scope collision: port '{port}' of sheet '{}' is also a global net",
                        def.name
                    ),
                ));
            }
        }
        for port in inst.connections.keys() {
            if !ports.contains(port) {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!(
                        "Sheet instance '{path}' connects '{port}', which is not a port of sheet '{}'",
                        def.name
                    ),
                ));
            }
        }
        for port in &ports {
            if !binding.contains_key(port) {
                self.diagnostics.push(diag(
                    ErcSeverity::Warning,
                    format!(
                        "Hierarchical port '{port}' of sheet instance '{path}' is not connected"
                    ),
                ));
            }
        }
        if def.body.hierarchy.is_some() {
            self.diagnostics.push(diag(
                ErcSeverity::Error,
                format!(
                    "Sheet '{}' body has its own hierarchy — nest sheets with `instances` instead (ignored)",
                    def.name
                ),
            ));
        }

        let globals = self.globals.clone();
        let resolve = |name: &str| -> String {
            if ports.contains(name) {
                if let Some(bound) = binding.get(name) {
                    return bound.clone();
                }
            } else if globals.contains(name) {
                return name.to_string();
            }
            format!("{path}/{name}")
        };

        // Reference designators.
        let repeated = self.use_count.get(&def.name).copied().unwrap_or(0) > 1;
        let mut refs = BTreeMap::new();
        for comp in &def.body.components {
            let flat_ref =
                if let Some(r) = inst.ref_map.as_ref().and_then(|m| m.get(&comp.reference)) {
                    r.clone()
                } else if let Some(off) = inst.ref_offset {
                    offset_ref(&comp.reference, off)
                } else if repeated {
                    format!("{}_{}", comp.reference, inst.name)
                } else {
                    comp.reference.clone()
                };
            refs.insert(comp.reference.clone(), flat_ref.clone());
            let mut c = comp.clone();
            c.reference = flat_ref;
            c.position = Vec2::new(c.position.x + offset.x, c.position.y + offset.y);
            self.out_components.push(c);
        }

        // Connectivity: the child's own netlist (wires, labels, explicit nets).
        let mut body = def.body.clone();
        body.hierarchy = None;
        let netlist = generate_netlist(&body);
        let mut used_names = BTreeSet::new();
        for net in &netlist.nets {
            if net.connections.is_empty() {
                continue;
            }
            let auto = is_auto_name(&net.name);
            if auto && net.connections.len() < 2 {
                continue;
            }
            used_names.insert(net.name.clone());
            let flat = resolve(&net.name);
            if !ports.contains(&net.name)
                && !self.globals.contains(&net.name)
                && self.root_names.contains(&flat)
            {
                self.diagnostics.push(diag(
                    ErcSeverity::Error,
                    format!(
                        "Net scope collision: local net '{}' of sheet instance '{path}' flattens to '{flat}', which the root sheet also uses",
                        net.name
                    ),
                ));
            }
            let entry = self.out_nets.entry(flat).or_default();
            for c in &net.connections {
                let r = refs.get(&c.component_ref).unwrap_or(&c.component_ref);
                entry.push(format!("{r}.{}", c.pin_number));
            }
        }
        // A port passed straight down to a child instance is used too.
        for child in &def.instances {
            used_names.extend(child.connections.values().cloned());
        }
        for port in &ports {
            if !used_names.contains(port)
                && self
                    .unused_port_reported
                    .insert(def.name.clone() + "\0" + port)
            {
                self.diagnostics.push(diag(
                    ErcSeverity::Warning,
                    format!(
                        "Hierarchical port '{port}' of sheet '{}' connects to nothing inside the sheet",
                        def.name
                    ),
                ));
            }
        }
        for flag in &def.body.power_flags {
            self.out_flags.push(PowerFlag {
                net: resolve(&flag.net),
                position: flag.position,
                note: flag.note.clone(),
            });
        }

        self.instances.push(FlatInstance {
            path: path.to_string(),
            sheet: def.name.clone(),
            refs,
        });

        self.elaborate_all(&def.instances, path, offset, &resolve, stack);
    }
}

/// Flatten a (possibly) hierarchical sheet. A sheet without a hierarchy is
/// returned unchanged with no diagnostics.
pub fn flatten(sheet: &SchematicSheet) -> Flattened {
    let Some(h) = sheet.hierarchy.as_deref() else {
        return Flattened {
            sheet: sheet.clone(),
            diagnostics: Vec::new(),
            instances: Vec::new(),
        };
    };

    let mut diagnostics = Vec::new();
    let mut defs: HashMap<&str, &SheetDefinition> = HashMap::new();
    for def in &h.sheets {
        if defs.insert(def.name.as_str(), def).is_some() {
            diagnostics.push(diag(
                ErcSeverity::Error,
                format!("Duplicate sheet definition '{}'", def.name),
            ));
        }
    }

    let mut globals: BTreeSet<String> = h.global_nets.iter().cloned().collect();
    let all_labels = sheet
        .labels
        .iter()
        .chain(h.sheets.iter().flat_map(|d| d.body.labels.iter()));
    for l in all_labels {
        if l.scope == LabelScope::Global {
            globals.insert(l.name.clone());
        }
    }

    let mut root_names: BTreeSet<String> = sheet.labels.iter().map(|l| l.name.clone()).collect();
    if let Some(n) = &sheet.nets {
        root_names.extend(n.keys().cloned());
    }

    let mut use_count = HashMap::new();
    count_uses(&defs, &h.instances, &mut use_count, &mut Vec::new());

    let mut root = sheet.clone();
    root.hierarchy = None;

    let mut ctx = Ctx {
        defs,
        globals,
        root_names,
        use_count,
        unused_port_reported: BTreeSet::new(),
        auto_slot: 0,
        out_components: Vec::new(),
        out_nets: root.nets.clone().unwrap_or_default(),
        out_flags: Vec::new(),
        instances: Vec::new(),
        diagnostics,
    };
    let identity = |n: &str| n.to_string();
    ctx.elaborate_all(
        &h.instances,
        "",
        Vec2::new(0.0, 0.0),
        &identity,
        &mut Vec::new(),
    );

    root.components.extend(ctx.out_components);
    for pins in ctx.out_nets.values_mut() {
        let mut seen = BTreeSet::new();
        pins.retain(|p| seen.insert(p.clone()));
    }
    root.nets = if ctx.out_nets.is_empty() {
        None
    } else {
        Some(ctx.out_nets)
    };
    root.power_flags.extend(ctx.out_flags);

    Flattened {
        sheet: root,
        diagnostics: ctx.diagnostics,
        instances: ctx.instances,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(number: &str, pin_type: PinType) -> SchematicPin {
        SchematicPin {
            number: number.into(),
            name: number.into(),
            pin_type,
            position: Vec2::new(number.parse::<f64>().unwrap_or(0.0) * 10.0, 0.0),
        }
    }

    fn comp(reference: &str, pins: Vec<SchematicPin>) -> SchematicComponent {
        SchematicComponent {
            reference: reference.into(),
            value: "x".into(),
            footprint_id: "R_0402".into(),
            position: Vec2::new(0.0, reference.bytes().map(f64::from).sum::<f64>() * 100.0),
            rotation: 0.0,
            mirror: false,
            pins,
            pads_override: None,
            properties: HashMap::new(),
        }
    }

    fn sheet(components: Vec<SchematicComponent>, nets: &[(&str, &[&str])]) -> SchematicSheet {
        SchematicSheet {
            title: None,
            components,
            wires: vec![],
            junctions: vec![],
            labels: vec![],
            nets: Some(
                nets.iter()
                    .map(|(n, p)| (n.to_string(), p.iter().map(|s| s.to_string()).collect()))
                    .collect(),
            ),
            power_flags: vec![],
            hierarchy: None,
        }
    }

    fn inst(name: &str, conns: &[(&str, &str)]) -> SheetInstance {
        SheetInstance {
            name: name.into(),
            sheet: "can".into(),
            connections: conns
                .iter()
                .map(|(a, b)| (a.to_string(), b.to_string()))
                .collect(),
            ref_map: None,
            ref_offset: None,
            position: None,
        }
    }

    /// A CAN transceiver sheet: U1 (VCC, GND, TXD, CANH) + termination R1.
    fn can_def() -> SheetDefinition {
        SheetDefinition {
            name: "can".into(),
            ports: vec![HierarchicalPort {
                name: "TXD".into(),
                direction: PinType::Input,
            }],
            body: sheet(
                vec![
                    comp(
                        "U1",
                        vec![
                            pin("1", PinType::PowerInput),
                            pin("2", PinType::PowerInput),
                            pin("3", PinType::Input),
                            pin("4", PinType::Bidirectional),
                        ],
                    ),
                    comp(
                        "R1",
                        vec![pin("1", PinType::Passive), pin("2", PinType::Passive)],
                    ),
                ],
                &[
                    ("VCC", &["U1.1"]),
                    ("GND", &["U1.2", "R1.2"]),
                    ("TXD", &["U1.3"]),
                    ("CANH", &["U1.4", "R1.1"]),
                ],
            ),
            instances: vec![],
        }
    }

    fn design(instances: Vec<SheetInstance>) -> SchematicSheet {
        let mut root = sheet(
            vec![comp(
                "U10",
                vec![
                    pin("1", PinType::Output),
                    pin("2", PinType::Output),
                    pin("3", PinType::PowerOutput),
                    pin("4", PinType::PowerInput),
                ],
            )],
            &[
                ("TX_A", &["U10.1"]),
                ("TX_B", &["U10.2"]),
                ("VCC", &["U10.3"]),
                ("GND", &["U10.4"]),
            ],
        );
        root.hierarchy = Some(Box::new(SchematicHierarchy {
            sheets: vec![can_def()],
            instances,
            global_nets: vec!["VCC".into(), "GND".into()],
        }));
        root
    }

    #[test]
    fn flat_sheet_passes_through() {
        let s = sheet(vec![comp("R1", vec![pin("1", PinType::Passive)])], &[]);
        let f = flatten(&s);
        assert_eq!(f.sheet, s);
        assert!(f.diagnostics.is_empty());
    }

    #[test]
    fn repeated_instances_scope_local_nets_and_refs() {
        let f = flatten(&design(vec![
            inst("CAN1", &[("TXD", "TX_A")]),
            inst("CAN2", &[("TXD", "TX_B")]),
        ]));
        assert!(f.diagnostics.is_empty(), "{:?}", f.diagnostics);
        let nets = f.sheet.nets.as_ref().unwrap();
        // Ports land on the parent's nets.
        assert_eq!(nets["TX_A"], vec!["U10.1", "U1_CAN1.3"]);
        assert_eq!(nets["TX_B"], vec!["U10.2", "U1_CAN2.3"]);
        // Globals are shared.
        assert_eq!(nets["VCC"], vec!["U10.3", "U1_CAN1.1", "U1_CAN2.1"]);
        // Locals are per instance.
        assert_eq!(nets["CAN1/CANH"], vec!["R1_CAN1.1", "U1_CAN1.4"]);
        assert_eq!(nets["CAN2/CANH"], vec!["R1_CAN2.1", "U1_CAN2.4"]);
        assert!(!nets.contains_key("CANH"));
        assert_eq!(f.sheet.components.len(), 5);
        assert!(f.sheet.hierarchy.is_none());
        assert_eq!(f.instances.len(), 2);
    }

    #[test]
    fn ref_offset_and_map() {
        let mut a = inst("CAN1", &[("TXD", "TX_A")]);
        a.ref_offset = Some(100);
        let mut b = inst("CAN2", &[("TXD", "TX_B")]);
        b.ref_map = Some([("U1".to_string(), "U7".to_string())].into());
        b.ref_offset = Some(200);
        let f = flatten(&design(vec![a, b]));
        let refs: Vec<_> = f
            .sheet
            .components
            .iter()
            .map(|c| c.reference.as_str())
            .collect();
        assert_eq!(refs, vec!["U10", "U101", "R101", "U7", "R201"]);
        assert_eq!(offset_ref("J", 5), "J5");
        assert_eq!(offset_ref("C12", 100), "C112");
    }

    #[test]
    fn port_diagnostics() {
        let f = flatten(&design(vec![
            inst("CAN1", &[]),
            inst("CAN2", &[("TXD", "TX_B"), ("RXD", "RX")]),
            SheetInstance {
                sheet: "nope".into(),
                ..inst("X", &[])
            },
        ]));
        let msgs: Vec<_> = f.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("port 'TXD' of sheet instance 'CAN1' is not connected")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|m| m.contains("connects 'RXD', which is not a port")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("unknown sheet 'nope'")),
            "{msgs:?}"
        );
        // The unbound port still forms a (local) net.
        assert!(f.sheet.nets.unwrap().contains_key("CAN1/TXD"));
    }

    #[test]
    fn scope_collisions() {
        let mut d = design(vec![inst("CAN1", &[("TXD", "TX_A")])]);
        d.nets
            .as_mut()
            .unwrap()
            .insert("CAN1/CANH".into(), vec!["U10.4".into()]);
        let h = d.hierarchy.as_mut().unwrap();
        h.global_nets.push("TXD".into());
        let f = flatten(&d);
        let msgs: Vec<_> = f.diagnostics.iter().map(|d| d.message.as_str()).collect();
        assert!(
            msgs.iter()
                .any(|m| m.contains("port 'TXD' of sheet 'can' is also a global net")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|m| m.contains("flattens to 'CAN1/CANH'")),
            "{msgs:?}"
        );
    }

    #[test]
    fn recursion_is_reported() {
        let mut d = design(vec![inst("CAN1", &[("TXD", "TX_A")])]);
        let h = d.hierarchy.as_mut().unwrap();
        h.sheets[0].instances.push(inst("INNER", &[]));
        let f = flatten(&d);
        assert!(f
            .diagnostics
            .iter()
            .any(|d| d.message.contains("Sheet recursion")));
    }

    #[test]
    fn nested_ports_resolve_through_parents_and_flags_scope() {
        // top → mid (port P bound to root TX_A) → can (TXD bound to mid's P).
        let mut d = design(vec![SheetInstance {
            sheet: "mid".into(),
            ..inst("M", &[("P", "TX_A")])
        }]);
        let h = d.hierarchy.as_mut().unwrap();
        let mut body = sheet(vec![], &[]);
        body.power_flags.push(PowerFlag {
            net: "LOCAL_RAIL".into(),
            position: None,
            note: None,
        });
        h.sheets.push(SheetDefinition {
            name: "mid".into(),
            ports: vec![HierarchicalPort {
                name: "P".into(),
                direction: PinType::Input,
            }],
            body,
            instances: vec![inst("C", &[("TXD", "P")])],
        });
        let f = flatten(&d);
        let nets = f.sheet.nets.as_ref().unwrap();
        assert_eq!(nets["TX_A"], vec!["U10.1", "U1.3"]);
        assert!(nets.contains_key("M/C/CANH"));
        assert_eq!(f.sheet.power_flags[0].net, "M/LOCAL_RAIL");
        assert_eq!(f.instances[1].path, "M/C");
    }
}
