//! URDF file reader: converts URDF XML to vcad Document.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use vcad_ir::{
    CsgOp, Document, InertialProperties, Instance, Joint as VcadJoint, JointKind, MaterialDef,
    Node, NodeId, PartDef, SceneEntry, Vec3,
};

use crate::error::UrdfError;
use crate::types::{Geometry, Joint, Link, Robot};

/// Locate the byte position just past `<robot ...>` and the position of
/// `</robot>` (start of close tag). Used by [`normalize_robot_child_order`].
fn locate_robot_bounds(reader: &mut quick_xml::Reader<&[u8]>) -> Result<(usize, usize), UrdfError> {
    use quick_xml::events::Event;
    let mut robot_open_end: Option<usize> = None;
    let mut depth: i32 = 0;
    loop {
        let pos_before = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|e| UrdfError::InvalidFormat(format!("xml scan: {e}")))?
        {
            Event::Start(e) => {
                if e.name().as_ref() == b"robot" && robot_open_end.is_none() {
                    robot_open_end = Some(reader.buffer_position() as usize);
                    depth = 1;
                } else if robot_open_end.is_some() {
                    depth += 1;
                }
            }
            Event::End(e) => {
                if let Some(open_end) = robot_open_end {
                    depth -= 1;
                    if depth == 0 && e.name().as_ref() == b"robot" {
                        return Ok((open_end, pos_before));
                    }
                }
            }
            Event::Eof => {
                return Err(UrdfError::InvalidFormat(if robot_open_end.is_some() {
                    "URDF has unclosed <robot>".into()
                } else {
                    "URDF has no <robot> root element".into()
                }));
            }
            _ => {}
        }
    }
}

/// Group the immediate children of the element whose body spans
/// `[body_start, body_end)` of `xml`, bucketing each child by tag name
/// into the order given by `order` (unlisted tags keep their relative
/// order and go last). Returns the rebuilt body, or `None` when the body
/// has no element children worth reordering.
///
/// Shared by the `<robot>` and `<link>` passes — both exist for the same
/// quick-xml reason described on [`normalize_robot_child_order`].
fn group_children_by_tag(
    xml: &str,
    body_start: usize,
    body_end: usize,
    order: &[&[u8]],
) -> Result<Vec<Vec<String>>, UrdfError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    // One bucket per entry in `order`, plus a trailing bucket for
    // anything unrecognized.
    let mut buckets: Vec<Vec<String>> = vec![Vec::new(); order.len() + 1];

    let mut reader = Reader::from_str(&xml[body_start..body_end]);
    reader.config_mut().trim_text(false);

    loop {
        let pos_before = reader.buffer_position() as usize + body_start;
        let event = reader
            .read_event()
            .map_err(|e| UrdfError::InvalidFormat(format!("xml scan: {e}")))?;
        let tag = match &event {
            Event::Start(e) => {
                let tag = e.name().as_ref().to_vec();
                reader
                    .read_to_end(e.name())
                    .map_err(|err| UrdfError::InvalidFormat(format!("read_to_end: {err}")))?;
                tag
            }
            Event::Empty(e) => e.name().as_ref().to_vec(),
            Event::Eof => break,
            // Comments / text / whitespace between children are dropped by
            // the reorder — they get folded into the gaps between elements
            // in the rebuilt string. This is intended, not an oversight:
            // URDF carries no significant whitespace or text content, and the
            // rebuilt string exists only to be handed to the serde parse.
            //
            // The one place a dropped comment *would* matter is
            // `commented_out_floating_joint`, which reads a floating joint out
            // of a comment. It is safe because it scans the **original** xml —
            // `read_urdf_from_str_with_options` passes `xml`, not `normalized`,
            // to `apply_floating_base`. Keep it that way.
            _ => continue,
        };
        let pos_after = reader.buffer_position() as usize + body_start;
        let slot = order
            .iter()
            .position(|t| *t == tag.as_slice())
            .unwrap_or(order.len());
        buckets[slot].push(xml[pos_before..pos_after].to_string());
    }

    Ok(buckets)
}

/// Reorder the children of a single `<link>` so all `<visual>` siblings
/// are contiguous and all `<collision>` siblings are contiguous.
///
/// Same quick-xml constraint as [`normalize_robot_child_order`], one level
/// down. URDF puts no ordering constraint on a link's children, and a link
/// whose collision geometry is a convex decomposition routinely interleaves
/// the two — XLeRobot's `base_link` is `collision`×4, `visual`×3,
/// `collision`×4, which parsed as a duplicate `collision` field before this.
fn normalize_link_child_order(link_xml: &str) -> Result<String, UrdfError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    // Find the end of the `<link ...>` open tag and the start of `</link>`.
    let mut reader = Reader::from_str(link_xml);
    reader.config_mut().trim_text(false);
    let mut body_start: Option<usize> = None;
    let mut depth: i32 = 0;
    let body_end;
    loop {
        let pos_before = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|e| UrdfError::InvalidFormat(format!("xml scan: {e}")))?
        {
            Event::Start(e) => {
                if e.name().as_ref() == b"link" && body_start.is_none() {
                    body_start = Some(reader.buffer_position() as usize);
                    depth = 1;
                } else if body_start.is_some() {
                    depth += 1;
                }
            }
            Event::End(e) => {
                if body_start.is_some() {
                    depth -= 1;
                    if depth == 0 && e.name().as_ref() == b"link" {
                        body_end = pos_before;
                        break;
                    }
                }
            }
            // A self-closing `<link name="x"/>` has no children to reorder.
            Event::Eof => return Ok(link_xml.to_string()),
            _ => {}
        }
    }
    let Some(body_start) = body_start else {
        return Ok(link_xml.to_string());
    };

    let buckets = group_children_by_tag(
        link_xml,
        body_start,
        body_end,
        &[b"inertial", b"visual", b"collision"],
    )?;

    let mut out = String::with_capacity(link_xml.len());
    out.push_str(&link_xml[..body_start]);
    for bucket in &buckets {
        for s in bucket {
            out.push('\n');
            out.push_str(s);
        }
    }
    out.push('\n');
    out.push_str(&link_xml[body_end..]);
    Ok(out)
}

/// Reorder top-level children of `<robot>` so all `<link>` siblings come
/// before all `<joint>` siblings, and normalize each link's own children.
///
/// quick-xml's serde adapter (as of 0.37) treats repeated siblings as
/// `Vec<T>` only when they're contiguous; an interleaved `<link><joint>
/// <link>` produces a "duplicate field" error. Real URDFs in the wild
/// (e.g. Unitree's official g1_23dof.urdf) do interleave heavily, so we
/// run a quick pre-pass that sorts the immediate children of `<robot>`
/// while preserving everything else byte-for-byte.
fn normalize_robot_child_order(xml: &str) -> Result<String, UrdfError> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    // Walk to <robot>. Everything before it (XML prolog, comments, top-
    // level whitespace) is preserved verbatim.
    // Pass 1: find <robot> opening end + </robot> position.
    let (robot_open_end, robot_close_start) = locate_robot_bounds(&mut reader)?;

    // Pass 2: walk inside <robot>, classify each top-level child by tag
    // name, and capture its byte range in the original source.
    let mut links: Vec<&str> = Vec::new();
    let mut joints: Vec<&str> = Vec::new();
    let mut materials: Vec<&str> = Vec::new();
    let mut others: Vec<&str> = Vec::new();

    let mut reader = Reader::from_str(&xml[robot_open_end..robot_close_start]);
    reader.config_mut().trim_text(false);
    let inner_offset = robot_open_end;

    loop {
        let pos_before = reader.buffer_position() as usize + inner_offset;
        match reader
            .read_event()
            .map_err(|e| UrdfError::InvalidFormat(format!("xml scan: {e}")))?
        {
            Event::Start(e) => {
                let tag = e.name().as_ref().to_vec();
                reader
                    .read_to_end(e.name())
                    .map_err(|err| UrdfError::InvalidFormat(format!("read_to_end: {err}")))?;
                let pos_after = reader.buffer_position() as usize + inner_offset;
                let slice = &xml[pos_before..pos_after];
                match tag.as_slice() {
                    b"link" => links.push(slice),
                    b"joint" => joints.push(slice),
                    b"material" => materials.push(slice),
                    _ => others.push(slice),
                }
            }
            Event::Empty(e) => {
                let tag = e.name().as_ref().to_vec();
                let pos_after = reader.buffer_position() as usize + inner_offset;
                let slice = &xml[pos_before..pos_after];
                match tag.as_slice() {
                    b"link" => links.push(slice),
                    b"joint" => joints.push(slice),
                    b"material" => materials.push(slice),
                    _ => others.push(slice),
                }
            }
            Event::Eof => break,
            // Comments / text / whitespace at the top level are dropped by
            // the reorder — they get folded into the gaps between elements
            // in the rebuilt string. URDF semantics don't care about
            // top-level whitespace.
            _ => {}
        }
    }

    // Rebuild: prefix (everything up to and including the <robot ...>
    // opening tag) verbatim, then materials, links, joints, others, then
    // the </robot> closing tag and any trailing content verbatim.
    let prefix = &xml[..robot_open_end];
    let mut out = String::with_capacity(xml.len());
    out.push_str(prefix);
    for s in &materials {
        out.push('\n');
        out.push_str(s);
    }
    for s in &links {
        out.push('\n');
        // Each link's own children need the same contiguity treatment.
        out.push_str(&normalize_link_child_order(s)?);
    }
    for s in &joints {
        out.push('\n');
        out.push_str(s);
    }
    for s in &others {
        out.push('\n');
        out.push_str(s);
    }
    out.push('\n');
    out.push_str(&xml[robot_close_start..]);
    Ok(out)
}

/// Options for resolving URDF `<mesh>` references.
///
/// Controls how `package://NAME/path` URIs and bare relative paths are
/// turned into absolute filesystem paths the physics layer can `open()`.
#[derive(Debug, Clone, Default)]
pub struct UrdfReadOptions {
    /// Directories to search for `package://NAME/...` resolution. Each
    /// root is checked for a `NAME/` subdirectory containing the rest of
    /// the URI; the first match wins.
    pub package_roots: Vec<PathBuf>,
    /// Directory the URDF lives in — used as the base for resolving
    /// relative mesh paths. Set automatically by [`read_urdf`].
    pub urdf_dir: Option<PathBuf>,
    /// Emit `MeshImport` paths relative to this directory instead of absolute.
    ///
    /// The importer's default is an absolute path, because the immediate
    /// consumer (physics, CSG eval) opens it with no base directory to resolve
    /// against. That default cannot be committed: an absolute path resolves
    /// only on the machine that ran the import, so a checked-in sample renders
    /// on exactly one computer.
    ///
    /// Set this to the *output document's* directory to get a portable
    /// document. Relative paths are then resolved against the document's own
    /// location by [`vcad_eval::resolve_mesh_paths`], which every loader that
    /// reads a `.vcad` from disk applies.
    pub mesh_paths_relative_to: Option<PathBuf>,
    /// Synthesize a floating base when the URDF has no floating joint.
    ///
    /// Most humanoid/quadruped URDFs ship the `world` link and its
    /// `type="floating"` joint **commented out**, because the convention is
    /// that the simulator supplies the free base (MuJoCo's MJCF for the same
    /// robot carries `<joint type="free"/>`). Without it vcad grounds the
    /// tree's root link and the robot is welded to the world — physically
    /// pinned, useless for locomotion. Setting this injects exactly the
    /// commented-out block: a geometry-less `world` link plus a
    /// [`JointKind::Free`] joint from it to the root link.
    ///
    /// No-op when the URDF already declares a floating joint.
    pub floating_base: bool,
    /// Link to attach the synthesized floating base to. Defaults to the
    /// tree's root link (the one that is never a joint's child). Ignored
    /// unless [`Self::floating_base`] is set.
    pub floating_base_link: Option<String>,
    /// Initial base height in **millimetres**, written as the synthesized
    /// joint's origin `z`.
    ///
    /// A `Free` joint's `state` is a scalar and therefore meaningless for
    /// 6 DOF, so `parentAnchor.z` is what actually sets the spawn height.
    /// Spawn slightly above the settled standing height so the robot drops
    /// onto the ground rather than starting interpenetrated (for the Booster
    /// K1, 620 mm against a 549.8 mm settled stand). Defaults to 0.
    pub spawn_height_mm: Option<f64>,
}

impl UrdfReadOptions {
    /// Resolve a URDF `<mesh filename="...">` value to an absolute path on
    /// disk, or `None` if no candidate exists. Logs (via `eprintln!`) when
    /// a `package://` URI cannot be located so the caller knows a mesh
    /// fell back to a placeholder.
    /// Render a resolved mesh path for writing into a `MeshImport` node:
    /// relative to [`Self::mesh_paths_relative_to`] when that is set and a
    /// relative path exists, absolute otherwise.
    pub fn render_mesh_path(&self, resolved: &Path) -> String {
        let Some(base) = self.mesh_paths_relative_to.as_ref() else {
            return resolved.to_string_lossy().into_owned();
        };
        match relative_path(resolved, base) {
            Some(rel) => rel.to_string_lossy().into_owned(),
            // No relative route (different volumes, say). Absolute beats a
            // path that silently resolves against the wrong directory.
            None => resolved.to_string_lossy().into_owned(),
        }
    }

    /// Resolve `filename` (a `package://`, `file://`, absolute or relative
    /// URDF mesh reference) to a real file on disk.
    pub fn resolve_mesh(&self, filename: &str) -> Option<PathBuf> {
        // package://NAME/rest/of/path
        if let Some(rest) = filename.strip_prefix("package://") {
            let mut split = rest.splitn(2, '/');
            let pkg = split.next()?;
            let sub = split.next().unwrap_or("");
            for root in &self.package_roots {
                let candidate = root.join(pkg).join(sub);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
            eprintln!(
                "urdf: package URI {filename:?} not found under {:?}",
                self.package_roots
            );
            return None;
        }
        // file:// or absolute or relative
        let stripped = filename.strip_prefix("file://").unwrap_or(filename);
        let p = Path::new(stripped);
        if p.is_absolute() {
            return p.is_file().then(|| p.to_path_buf());
        }
        if let Some(dir) = &self.urdf_dir {
            let candidate = dir.join(p);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
        None
    }
}

/// Maximum URDF size we will attempt to parse. URDFs are tiny descriptors
/// (tens of KB); a multi-MB "URDF" is almost always an attack payload.
const MAX_URDF_BYTES: usize = 8 * 1024 * 1024;
/// Caps on structural count to bound post-parse work.
const MAX_LINKS: usize = 10_000;
const MAX_JOINTS: usize = 10_000;

/// Read a URDF file from a path with default mesh-resolution options
/// (mesh paths are resolved relative to the URDF's parent directory; no
/// `package://` roots are configured).
pub fn read_urdf(path: impl AsRef<Path>) -> Result<Document, UrdfError> {
    let path = path.as_ref();
    let opts = UrdfReadOptions {
        urdf_dir: path.parent().map(|p| p.to_path_buf()),
        ..UrdfReadOptions::default()
    };
    read_urdf_with_options(path, &opts)
}

/// Read a URDF file from a path, providing explicit mesh-resolution
/// options (e.g. `package_roots` for `package://NAME/...` URIs).
pub fn read_urdf_with_options(
    path: impl AsRef<Path>,
    opts: &UrdfReadOptions,
) -> Result<Document, UrdfError> {
    let path = path.as_ref();
    let content = std::fs::read_to_string(path)?;
    let mut effective = opts.clone();
    if effective.urdf_dir.is_none() {
        effective.urdf_dir = path.parent().map(|p| p.to_path_buf());
    }
    read_urdf_from_str_with_options(&content, &effective)
}

/// Read a URDF from a string with default options. Use
/// [`read_urdf_from_str_with_options`] when meshes need to be resolved
/// against package roots.
pub fn read_urdf_from_str(xml: &str) -> Result<Document, UrdfError> {
    read_urdf_from_str_with_options(xml, &UrdfReadOptions::default())
}

/// Read a URDF from a string with explicit mesh-resolution options.
pub fn read_urdf_from_str_with_options(
    xml: &str,
    opts: &UrdfReadOptions,
) -> Result<Document, UrdfError> {
    if xml.len() > MAX_URDF_BYTES {
        return Err(UrdfError::InvalidFormat(format!(
            "URDF exceeds {} byte limit",
            MAX_URDF_BYTES
        )));
    }
    // Reject DOCTYPE outright. quick-xml 0.37 does not expand entity
    // references, but a DOCTYPE is otherwise never needed in URDF and
    // rejecting it provides defense-in-depth against XXE / billion-laughs
    // if a future quick-xml release (or a different parser) ever starts
    // expanding them.
    if contains_doctype(xml) {
        return Err(UrdfError::InvalidFormat(
            "URDF contains a DOCTYPE declaration (rejected)".into(),
        ));
    }
    // Real-world URDFs interleave <link> and <joint> siblings; quick-xml's
    // serde adapter only deserialises Vec<T> when those siblings are
    // contiguous, so reorder them up front. The normalisation is a no-op
    // when the file is already in canonical (links-then-joints) order.
    let normalized = normalize_robot_child_order(xml)?;
    let robot: Robot = quick_xml::de::from_str(&normalized)?;
    if robot.links.len() > MAX_LINKS {
        return Err(UrdfError::InvalidFormat(format!(
            "URDF has {} links (cap {})",
            robot.links.len(),
            MAX_LINKS
        )));
    }
    if robot.joints.len() > MAX_JOINTS {
        return Err(UrdfError::InvalidFormat(format!(
            "URDF has {} joints (cap {})",
            robot.joints.len(),
            MAX_JOINTS
        )));
    }
    let mut robot = robot;
    apply_floating_base(&mut robot, opts, xml)?;
    warn_zero_authority_joints(&robot);
    let reader = UrdfReader::new(&robot, opts);
    reader.into_document()
}

/// Warn about movable joints whose `<limit>` declares `effort="0"`.
///
/// An effort limit of zero is a hard saturation at zero torque: the joint is
/// mechanically present and free to swing under gravity, but no controller
/// input can ever move it. That is almost never what the author meant — it is
/// what a CAD exporter writes when nobody filled the field in, and simulators
/// that take their actuator limits from a separate controller config (SAPIEN /
/// ManiSkill, MuJoCo keyframes) never read it, so the zero survives upstream
/// unnoticed.
///
/// It is not silently rewritten here: an effort limit is a declared physical
/// claim, and guessing a replacement would fabricate torque the description
/// does not authorize. The importer reports it and lets the caller supply real
/// limits. XLeRobot's `xlerobot.urdf` declares `effort="0"` on all twelve arm
/// joints — see `third_party/xlerobot/README.md`.
fn warn_zero_authority_joints(robot: &crate::types::Robot) {
    let zeroed: Vec<&str> = robot
        .joints
        .iter()
        .filter(|j| matches!(j.joint_type.trim(), "revolute" | "continuous" | "prismatic"))
        .filter(|j| j.limit.as_ref().and_then(|l| l.effort) == Some(0.0))
        .map(|j| j.name.as_str())
        .collect();

    if zeroed.is_empty() {
        return;
    }
    eprintln!(
        "urdf: '{}' declares effort=\"0\" on {} movable joint(s) — they will be \
         inert, because a zero effort limit saturates every controller output \
         to zero torque. Supply real actuator limits before simulating. \
         Affected: {}",
        robot.name,
        zeroed.len(),
        zeroed.join(", ")
    );
}

/// Name of the floating joint found inside a commented-out region of `xml`,
/// if any.
///
/// The near-universal convention is to ship the world link and its floating
/// joint commented out and let the simulator supply the free base. That
/// comment is a strong signal the author *wants* a floating base, so callers
/// can surface it when [`UrdfReadOptions::floating_base`] was not requested.
pub fn commented_out_floating_joint(xml: &str) -> Option<String> {
    let mut rest = xml;
    loop {
        let start = rest.find("<!--")?;
        let after = &rest[start + 4..];
        match after.find("-->") {
            Some(end) => {
                if let Some(name) = floating_joint_name_in(&after[..end]) {
                    return Some(name);
                }
                rest = &after[end + 3..];
            }
            // Unterminated comment: everything left is commented out.
            None => return floating_joint_name_in(after),
        }
    }
}

/// Scan a fragment of URDF-ish text for `<joint ... type="floating">` and
/// return the joint's `name` (or a placeholder when it has none). Purely
/// textual — the fragment lives inside a comment, so it need not be
/// well-formed enough to parse.
fn floating_joint_name_in(body: &str) -> Option<String> {
    let mut rest = body;
    while let Some(idx) = rest.find("<joint") {
        rest = &rest[idx + 6..];
        let tag_end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..tag_end];
        if attr_value(tag, "type").as_deref() == Some("floating") {
            return Some(attr_value(tag, "name").unwrap_or_else(|| "<unnamed>".to_string()));
        }
    }
    None
}

/// Extract `name="value"` (or `name='value'`) from an XML start-tag body.
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let needle = format!("{attr}={quote}");
        if let Some(i) = tag.find(&needle) {
            let after = &tag[i + needle.len()..];
            if let Some(j) = after.find(quote) {
                return Some(after[..j].to_string());
            }
        }
    }
    None
}

/// Inject the `world` link + `type="floating"` joint that the URDF left
/// commented out, when [`UrdfReadOptions::floating_base`] asks for it.
///
/// [`UrdfReadOptions::spawn_height_mm`] applies to the floating joint either
/// way — synthesized or already authored in the URDF.
///
/// When the option is *not* set but the XML's comments do contain a floating
/// joint, warn on stderr — that is the case this option exists for.
fn apply_floating_base(
    robot: &mut Robot,
    opts: &UrdfReadOptions,
    xml: &str,
) -> Result<(), UrdfError> {
    let already_floating = robot
        .joints
        .iter()
        .any(|j| j.joint_type.trim() == "floating");

    if !opts.floating_base {
        if already_floating {
            apply_spawn_height(robot, opts);
            return Ok(());
        }
        {
            if let Some(name) = commented_out_floating_joint(xml) {
                eprintln!(
                    "urdf: '{}' declares a floating joint ({name}) inside a comment — the \
                     robot will be welded to the world. Re-import with floating_base to \
                     synthesize it (CLI: --floating-base).",
                    robot.name
                );
            }
        }
        return Ok(());
    }

    if already_floating {
        apply_spawn_height(robot, opts);
        return Ok(());
    }

    let root = match &opts.floating_base_link {
        Some(link) => {
            if !robot.links.iter().any(|l| &l.name == link) {
                return Err(UrdfError::UnknownLink(link.clone()));
            }
            link.clone()
        }
        None => find_root_link_name(robot)?,
    };

    let world = unique_name("world", |n| robot.links.iter().any(|l| l.name == n));
    let joint_name = unique_name("world_joint", |n| robot.joints.iter().any(|j| j.name == n));

    robot.links.push(Link {
        name: world.clone(),
        visuals: Vec::new(),
        collisions: Vec::new(),
        inertial: None,
    });
    // URDF origins are metres; the IR converts ×1000 on the way in.
    let z_m = opts.spawn_height_mm.unwrap_or(0.0) / 1000.0;
    robot.joints.push(Joint {
        name: joint_name,
        joint_type: "floating".to_string(),
        origin: Some(crate::types::Origin {
            xyz: Some(format!("0 0 {z_m}")),
            rpy: None,
        }),
        parent: crate::types::ParentLink { link: world },
        child: crate::types::ChildLink { link: root },
        axis: None,
        limit: None,
        dynamics: None,
    });
    Ok(())
}

/// First `base` name not already taken, suffixing `_1`, `_2`, … as needed.
fn unique_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_string();
    }
    (1..)
        .map(|i| format!("{base}_{i}"))
        .find(|n| !taken(n))
        .unwrap()
}

/// The link that is never a joint's child — see
/// [`UrdfReader::find_root_link`], which does the same on the reader's
/// borrowed robot.
fn find_root_link_name(robot: &Robot) -> Result<String, UrdfError> {
    let children: std::collections::HashSet<_> =
        robot.joints.iter().map(|j| &j.child.link).collect();
    robot
        .links
        .iter()
        .find(|l| !children.contains(&l.name))
        .or_else(|| robot.links.first())
        .map(|l| l.name.clone())
        .ok_or_else(|| UrdfError::MissingElement("No links found".to_string()))
}

/// Case-insensitive scan for `<!DOCTYPE` that ignores leading whitespace.
fn contains_doctype(xml: &str) -> bool {
    // Walk through comments/whitespace and check for a DOCTYPE at the prolog.
    // A stricter parse isn't worth the cost — false positives here just make
    // us reject a document that would have been rejected anyway.
    let bytes = xml.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b' ' | b'\t' | b'\r' | b'\n' => i += 1,
            b'<' => {
                if bytes[i..].starts_with(b"<!--") {
                    if let Some(end) = xml[i..].find("-->") {
                        i += end + 3;
                        continue;
                    }
                    return false;
                }
                if bytes[i..].starts_with(b"<?") {
                    if let Some(end) = xml[i..].find("?>") {
                        i += end + 2;
                        continue;
                    }
                    return false;
                }
                if bytes.len() - i >= 9 && bytes[i..i + 9].eq_ignore_ascii_case(b"<!DOCTYPE") {
                    return true;
                }
                return false;
            }
            _ => return false,
        }
    }
    false
}

/// Context for reading URDF and building vcad Document.
struct UrdfReader<'a> {
    robot: &'a Robot,
    /// Maps link name to vcad part def ID.
    link_to_part: HashMap<String, String>,
    /// Maps link name to instance ID.
    link_to_instance: HashMap<String, String>,
    /// Next node ID.
    next_node_id: NodeId,
    /// Resolves `<mesh filename>` references to absolute paths.
    opts: &'a UrdfReadOptions,
}

impl<'a> UrdfReader<'a> {
    fn new(robot: &'a Robot, opts: &'a UrdfReadOptions) -> Self {
        Self {
            robot,
            link_to_part: HashMap::new(),
            link_to_instance: HashMap::new(),
            next_node_id: 1,
            opts,
        }
    }

    fn alloc_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    fn into_document(mut self) -> Result<Document, UrdfError> {
        let mut doc = Document::new();

        // Build material map from top-level materials
        let material_map = self.build_materials(&mut doc);

        // Build part definitions from links
        let mut part_defs = HashMap::new();
        for link in &self.robot.links {
            let (part_def, nodes) = self.link_to_part_def(link, &material_map)?;
            let part_id = part_def.id.clone();
            self.link_to_part.insert(link.name.clone(), part_id.clone());
            part_defs.insert(part_id, part_def);
            for (id, node) in nodes {
                doc.nodes.insert(id, node);
            }
        }
        doc.part_defs = Some(part_defs);

        // Find root link (link that is not a child of any joint)
        let root_link = self.find_root_link()?;

        // Build instances for all links
        let mut instances = Vec::new();
        for link in &self.robot.links {
            let part_id = self.link_to_part.get(&link.name).unwrap().clone();
            let instance_id = format!("{}_inst", link.name);
            self.link_to_instance
                .insert(link.name.clone(), instance_id.clone());

            instances.push(Instance {
                id: instance_id,
                part_def_id: part_id,
                name: Some(link.name.clone()),
                tags: Vec::new(),
                transform: None, // Transforms come from joints
                material: None,
            });
        }
        doc.instances = Some(instances);

        // Set ground instance
        doc.ground_instance_id = self.link_to_instance.get(&root_link).cloned();

        // Build joints
        let mut vcad_joints = Vec::new();
        for joint in &self.robot.joints {
            let vcad_joint = self.joint_to_vcad(joint)?;
            vcad_joints.push(vcad_joint);
        }
        doc.joints = Some(vcad_joints);

        // Add scene entries for each link's root geometry
        for link in &self.robot.links {
            if let Some(part_defs) = &doc.part_defs {
                if let Some(part_def) = part_defs.get(&self.link_to_part[&link.name]) {
                    doc.roots.push(SceneEntry {
                        root: part_def.root,
                        material: "default".to_string(),
                        visible: None,
                    });
                }
            }
        }

        Ok(doc)
    }

    fn build_materials(&self, doc: &mut Document) -> HashMap<String, String> {
        let mut map = HashMap::new();

        // Add default material
        doc.materials.insert(
            "default".to_string(),
            MaterialDef {
                name: "default".to_string(),
                color: [0.7, 0.7, 0.7],
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
                ..Default::default()
            },
        );

        // Add materials from URDF
        for mat in &self.robot.materials {
            let color = mat
                .color
                .as_ref()
                .map(|c| {
                    let rgba = c.rgba_vec();
                    [rgba[0], rgba[1], rgba[2]]
                })
                .unwrap_or([0.5, 0.5, 0.5]);

            let mat_def = MaterialDef {
                name: mat.name.clone(),
                color,
                metallic: 0.0,
                roughness: 0.5,
                density: None,
                friction: None,
                ..Default::default()
            };

            doc.materials.insert(mat.name.clone(), mat_def);
            map.insert(mat.name.clone(), mat.name.clone());
        }

        map
    }

    fn link_to_part_def(
        &mut self,
        link: &Link,
        _material_map: &HashMap<String, String>,
    ) -> Result<(PartDef, Vec<(NodeId, Node)>), UrdfError> {
        let mut nodes = Vec::new();

        // Every `<collision>` becomes its own DAG root. URDF links routinely
        // declare several — a convex decomposition of the link — and the
        // pieces exist precisely because the single visual mesh is a bad
        // collider, so they must stay separate rather than being unioned
        // back into one shape. `PartDef::colliders` carries the list; the
        // physics layer turns each entry into its own collider.
        let mut colliders = Vec::with_capacity(link.collisions.len());
        for (i, collision) in link.collisions.iter().enumerate() {
            let label = collision
                .name
                .clone()
                .unwrap_or_else(|| format!("{}_collision{i}", link.name));
            colliders.push(self.geometry_subtree(
                &collision.geometry,
                collision.origin.as_ref(),
                &label,
                &mut nodes,
            )?);
        }

        // Rendering geometry: the first `<visual>`. URDFs that ship multiple
        // visuals per link describe multi-mesh parts; the importer picks one
        // to render (full multi-visual support needs an IR grouping op —
        // `Union` is a boolean and would weld the meshes together). A link
        // with no visual falls back to its first collider root rather than
        // duplicating that subtree.
        let root_id = if let Some(visual) = link.visuals.first() {
            self.geometry_subtree(
                &visual.geometry,
                visual.origin.as_ref(),
                &link.name,
                &mut nodes,
            )?
        } else if let Some(first) = colliders.first() {
            *first
        } else {
            // Link with no geometry - create empty cube placeholder
            let node_id = self.alloc_node_id();
            nodes.push((
                node_id,
                Node {
                    id: node_id,
                    name: Some(link.name.clone()),
                    op: CsgOp::Cube {
                        size: Vec3::new(0.01, 0.01, 0.01), // 1cm placeholder
                    },
                },
            ));
            node_id
        };

        let inertial = link.inertial.as_ref().map(|i| {
            // URDF stores COM in metres relative to the link frame; the rest
            // of the IR uses millimetres.
            let com_xyz = i
                .origin
                .as_ref()
                .map(|o| o.xyz_vec())
                .unwrap_or([0.0, 0.0, 0.0]);
            let inertia = &i.inertia;
            InertialProperties {
                mass_kg: i.mass.value,
                com_mm: Vec3::new(
                    com_xyz[0] * 1000.0,
                    com_xyz[1] * 1000.0,
                    com_xyz[2] * 1000.0,
                ),
                // [ixx, iyy, izz, ixy, ixz, iyz]
                inertia_kg_m2: [
                    inertia.ixx,
                    inertia.iyy,
                    inertia.izz,
                    inertia.ixy,
                    inertia.ixz,
                    inertia.iyz,
                ],
            }
        });

        Ok((
            PartDef {
                id: format!("part_{}", link.name),
                name: Some(link.name.clone()),
                root: root_id,
                default_material: Some("default".to_string()),
                inertial,
                colliders: (!colliders.is_empty()).then_some(colliders),
            },
            nodes,
        ))
    }

    /// Build the node subtree for one URDF `<geometry>` + `<origin>` pair and
    /// return its root, appending every node it creates to `nodes`.
    ///
    /// Shared by `<visual>` and `<collision>`: the two elements have identical
    /// shape in URDF, and a collision piece needs exactly the same primitive
    /// re-centering and origin placement a visual does.
    fn geometry_subtree(
        &mut self,
        geom: &Geometry,
        origin: Option<&crate::types::Origin>,
        label: &str,
        nodes: &mut Vec<(NodeId, Node)>,
    ) -> Result<NodeId, UrdfError> {
        // Create geometry node
        let geom_node_id = self.alloc_node_id();
        let (geom_op, center_offset) = self.geometry_to_csg(geom)?;
        nodes.push((
            geom_node_id,
            Node {
                id: geom_node_id,
                name: Some(format!("{label}_geom")),
                op: geom_op,
            },
        ));

        // URDF box/cylinder/cone primitives are centered on the link frame
        // origin. vcad's Cube extends from the origin into the +X/+Y/+Z
        // octant, and Cylinder/Cone sit on z=0 → z=height. Wrap each
        // primitive in a Translate so the geometry lines up with the URDF
        // convention before any <origin> rotation/translation is applied.
        let centered_node_id = if center_offset.x.abs() > 1e-9
            || center_offset.y.abs() > 1e-9
            || center_offset.z.abs() > 1e-9
        {
            let id = self.alloc_node_id();
            nodes.push((
                id,
                Node {
                    id,
                    name: Some(format!("{label}_center")),
                    op: CsgOp::Translate {
                        child: geom_node_id,
                        offset: center_offset,
                    },
                },
            ));
            id
        } else {
            geom_node_id
        };

        // Apply origin transform if present
        let Some(origin) = origin else {
            return Ok(centered_node_id);
        };
        let xyz = origin.xyz_vec();
        let rpy = origin.rpy_vec();

        // URDF uses meters, vcad uses mm
        let xyz_mm = [xyz[0] * 1000.0, xyz[1] * 1000.0, xyz[2] * 1000.0];

        // URDF uses radians, vcad uses degrees
        let rpy_deg = [
            rpy[0].to_degrees(),
            rpy[1].to_degrees(),
            rpy[2].to_degrees(),
        ];

        let has_translation = xyz_mm.iter().any(|v| v.abs() > 1e-6);
        let has_rotation = rpy_deg.iter().any(|v| v.abs() > 1e-6);

        let rotated_id = if has_rotation {
            let rotate_id = self.alloc_node_id();
            nodes.push((
                rotate_id,
                Node {
                    id: rotate_id,
                    name: Some(format!("{label}_rotate")),
                    op: CsgOp::Rotate {
                        child: centered_node_id,
                        angles: Vec3::new(rpy_deg[0], rpy_deg[1], rpy_deg[2]),
                    },
                },
            ));
            rotate_id
        } else {
            centered_node_id
        };

        if !has_translation {
            return Ok(rotated_id);
        }
        let translate_id = self.alloc_node_id();
        nodes.push((
            translate_id,
            Node {
                id: translate_id,
                name: Some(format!("{label}_translate")),
                op: CsgOp::Translate {
                    child: rotated_id,
                    offset: Vec3::new(xyz_mm[0], xyz_mm[1], xyz_mm[2]),
                },
            },
        ));
        Ok(translate_id)
    }

    /// Convert a URDF `<geometry>` to a vcad `CsgOp` and return the
    /// translation needed to center it on the link frame origin.
    ///
    /// URDF primitives (box, cylinder, sphere) are defined centered at the
    /// link frame. vcad's kernel primitives are anchored differently:
    /// `Cube` puts its corner at the origin, `Cylinder`/`Cone` sit on the
    /// XY plane. Returning the centering offset lets the caller wrap the
    /// geometry node in a Translate so the URDF semantics survive.
    fn geometry_to_csg(&self, geom: &Geometry) -> Result<(CsgOp, Vec3), UrdfError> {
        if let Some(box_geom) = &geom.box_geom {
            let size = box_geom.size_vec();
            // URDF uses meters, vcad uses mm
            let sx = size[0] * 1000.0;
            let sy = size[1] * 1000.0;
            let sz = size[2] * 1000.0;
            Ok((
                CsgOp::Cube {
                    size: Vec3::new(sx, sy, sz),
                },
                Vec3::new(-sx / 2.0, -sy / 2.0, -sz / 2.0),
            ))
        } else if let Some(cyl) = &geom.cylinder {
            // URDF cylinder is along Z axis, centered; vcad cylinder sits
            // on the XY plane so shift it down by half its height.
            let h = cyl.length * 1000.0;
            Ok((
                CsgOp::Cylinder {
                    radius: cyl.radius * 1000.0,
                    height: h,
                    segments: 32,
                },
                Vec3::new(0.0, 0.0, -h / 2.0),
            ))
        } else if let Some(sphere) = &geom.sphere {
            Ok((
                CsgOp::Sphere {
                    radius: sphere.radius * 1000.0,
                    segments: 32,
                },
                Vec3::new(0.0, 0.0, 0.0),
            ))
        } else if let Some(mesh) = &geom.mesh {
            let scale = mesh
                .scale
                .as_ref()
                .map(|s| {
                    let parts: Vec<f64> = s
                        .split_whitespace()
                        .filter_map(|p| p.parse().ok())
                        .collect();
                    if parts.len() >= 3 {
                        Some(Vec3::new(parts[0], parts[1], parts[2]))
                    } else {
                        None
                    }
                })
                .unwrap_or(None);
            // Try to resolve against filesystem (CLI path with urdf_dir
            // set). If that succeeds, emit an absolute path so downstream
            // physics / CSG eval can `open()` the file. Otherwise emit
            // MeshImport with the original URDF filename verbatim — the
            // browser entry point loads meshes JS-side and post-processes
            // these nodes into `ImportedMesh`, and physics paths without a
            // filesystem just skip them.
            let path = self
                .opts
                .resolve_mesh(&mesh.filename)
                .map(|p| self.opts.render_mesh_path(&p))
                .unwrap_or_else(|| mesh.filename.clone());
            Ok((CsgOp::MeshImport { path, scale }, Vec3::new(0.0, 0.0, 0.0)))
        } else {
            Err(UrdfError::InvalidGeometry(
                "No geometry type specified".to_string(),
            ))
        }
    }

    fn find_root_link(&self) -> Result<String, UrdfError> {
        // Root link is one that is never a child in any joint
        let child_links: std::collections::HashSet<_> =
            self.robot.joints.iter().map(|j| &j.child.link).collect();

        for link in &self.robot.links {
            if !child_links.contains(&link.name) {
                return Ok(link.name.clone());
            }
        }

        // If no root found, use first link
        self.robot
            .links
            .first()
            .map(|l| l.name.clone())
            .ok_or_else(|| UrdfError::MissingElement("No links found".to_string()))
    }

    fn joint_to_vcad(&self, joint: &Joint) -> Result<VcadJoint, UrdfError> {
        let parent_instance_id = self.link_to_instance.get(&joint.parent.link).cloned();
        let child_instance_id = self
            .link_to_instance
            .get(&joint.child.link)
            .cloned()
            .ok_or_else(|| UrdfError::UnknownLink(joint.child.link.clone()))?;

        // Parse origin
        let origin = joint.origin.as_ref();
        let xyz = origin.map(|o| o.xyz_vec()).unwrap_or([0.0, 0.0, 0.0]);

        // URDF uses meters, vcad uses mm
        let parent_anchor = Vec3::new(xyz[0] * 1000.0, xyz[1] * 1000.0, xyz[2] * 1000.0);
        let child_anchor = Vec3::new(0.0, 0.0, 0.0);

        // Convert joint type
        let kind = match joint.joint_type.as_str() {
            "fixed" => JointKind::Fixed,
            "revolute" => {
                let axis = joint
                    .axis
                    .as_ref()
                    .map(|a| a.xyz_vec())
                    .unwrap_or([0.0, 0.0, 1.0]);
                let limits = joint.limit.as_ref().and_then(|l| {
                    match (l.lower, l.upper) {
                        (Some(lower), Some(upper)) => {
                            // URDF uses radians, vcad uses degrees
                            Some((lower.to_degrees(), upper.to_degrees()))
                        }
                        _ => None,
                    }
                });
                JointKind::Revolute {
                    axis: Vec3::new(axis[0], axis[1], axis[2]),
                    limits,
                    // URDF effort is already N·m; velocity is rad/s → deg/s
                    effort_limit: joint.limit.as_ref().and_then(|l| l.effort),
                    velocity_limit: joint
                        .limit
                        .as_ref()
                        .and_then(|l| l.velocity)
                        .map(f64::to_degrees),
                }
            }
            "continuous" => {
                // Continuous is revolute without limits
                let axis = joint
                    .axis
                    .as_ref()
                    .map(|a| a.xyz_vec())
                    .unwrap_or([0.0, 0.0, 1.0]);
                JointKind::Revolute {
                    axis: Vec3::new(axis[0], axis[1], axis[2]),
                    limits: None,
                    effort_limit: joint.limit.as_ref().and_then(|l| l.effort),
                    velocity_limit: joint
                        .limit
                        .as_ref()
                        .and_then(|l| l.velocity)
                        .map(f64::to_degrees),
                }
            }
            "prismatic" => {
                let axis = joint
                    .axis
                    .as_ref()
                    .map(|a| a.xyz_vec())
                    .unwrap_or([1.0, 0.0, 0.0]);
                let limits = joint.limit.as_ref().and_then(|l| {
                    match (l.lower, l.upper) {
                        (Some(lower), Some(upper)) => {
                            // URDF uses meters, vcad uses mm
                            Some((lower * 1000.0, upper * 1000.0))
                        }
                        _ => None,
                    }
                });
                JointKind::Slider {
                    axis: Vec3::new(axis[0], axis[1], axis[2]),
                    limits,
                    // URDF effort is already N; velocity is m/s → mm/s
                    effort_limit: joint.limit.as_ref().and_then(|l| l.effort),
                    velocity_limit: joint
                        .limit
                        .as_ref()
                        .and_then(|l| l.velocity)
                        .map(|v| v * 1000.0),
                }
            }
            "floating" => {
                // Full 6-DOF: the child (e.g. a humanoid's floating base)
                // can translate and rotate freely. Previously mapped to
                // Ball, which silently pinned the base in space.
                JointKind::Free
            }
            "planar" => {
                // Approximation: URDF planar is 2 translation + 1 rotation
                // DOF in the plane normal to `axis`. vcad has no planar
                // joint kind yet, so approximate with Ball (3 rotational
                // DOF about the anchor) to keep the link mobile without
                // letting it leave the anchor entirely.
                JointKind::Ball
            }
            other => return Err(UrdfError::UnsupportedJointType(other.to_string())),
        };

        Ok(VcadJoint {
            id: joint.name.clone(),
            name: Some(joint.name.clone()),
            parent_instance_id,
            child_instance_id,
            parent_anchor,
            child_anchor,
            kind,
            state: 0.0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_URDF: &str = r#"<?xml version="1.0"?>
<robot name="simple_robot">
    <link name="base_link">
        <visual>
            <geometry>
                <box size="0.1 0.1 0.05"/>
            </geometry>
        </visual>
    </link>
    <link name="arm_link">
        <visual>
            <origin xyz="0 0 0.05"/>
            <geometry>
                <cylinder radius="0.02" length="0.1"/>
            </geometry>
        </visual>
    </link>
    <joint name="base_to_arm" type="revolute">
        <parent link="base_link"/>
        <child link="arm_link"/>
        <origin xyz="0 0 0.025"/>
        <axis xyz="0 0 1"/>
        <limit lower="-1.57" upper="1.57" effort="10" velocity="1"/>
    </joint>
</robot>"#;

    /// A link whose `<collision>` children are split by `<visual>` ones.
    /// URDF puts no ordering constraint on a link's children, and a convex
    /// decomposition routinely produces this — XLeRobot's `base_link` is
    /// collision x4, visual x3, collision x4.
    const INTERLEAVED_URDF: &str = r#"<?xml version="1.0"?>
<robot name="interleaved">
    <link name="base_link">
        <inertial>
            <mass value="1.0"/>
            <inertia ixx="0.1" ixy="0" ixz="0" iyy="0.1" iyz="0" izz="0.1"/>
        </inertial>
        <collision><geometry><box size="0.1 0.1 0.1"/></geometry></collision>
        <collision><geometry><box size="0.2 0.2 0.2"/></geometry></collision>
        <visual><geometry><box size="0.3 0.3 0.3"/></geometry></visual>
        <visual><geometry><box size="0.4 0.4 0.4"/></geometry></visual>
        <collision><geometry><box size="0.5 0.5 0.5"/></geometry></collision>
    </link>
</robot>"#;

    #[test]
    fn parses_link_with_interleaved_visual_and_collision() {
        // quick-xml's serde adapter only folds *contiguous* repeated siblings
        // into a Vec, so this used to fail with "duplicate field `collision`"
        // and no document at all.
        let doc = read_urdf_from_str(INTERLEAVED_URDF)
            .expect("interleaved visual/collision children are legal URDF");
        assert_eq!(doc.part_defs.map_or(0, |p| p.len()), 1);
    }

    #[test]
    fn interleaved_children_are_all_preserved() {
        // Reordering must not drop any of them: parse the normalized XML back
        // and count. A normalizer that kept only the first run of each tag
        // would still satisfy the test above.
        let normalized = normalize_robot_child_order(INTERLEAVED_URDF).unwrap();
        let robot: Robot = quick_xml::de::from_str(&normalized).unwrap();
        let link = &robot.links[0];
        assert_eq!(link.collisions.len(), 3, "all three collisions survive");
        assert_eq!(link.visuals.len(), 2, "both visuals survive");
        assert!(link.inertial.is_some(), "the inertial survives");
    }

    #[test]
    fn warns_but_keeps_zero_effort_limits() {
        // effort="0" is a real declaration (an inert joint) and must not be
        // silently rewritten — the importer only reports it.
        let urdf = SIMPLE_URDF.replace(r#"effort="10""#, r#"effort="0""#);
        let doc = read_urdf_from_str(&urdf).unwrap();
        let joint = &doc.joints.unwrap()[0];
        match &joint.kind {
            JointKind::Revolute { effort_limit, .. } => {
                assert_eq!(*effort_limit, Some(0.0), "the declared zero is preserved");
            }
            other => panic!("expected a revolute joint, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_simple_urdf() {
        let doc = read_urdf_from_str(SIMPLE_URDF).unwrap();

        // Check basic structure
        assert!(doc.part_defs.is_some());
        assert!(doc.instances.is_some());
        assert!(doc.joints.is_some());

        let part_defs = doc.part_defs.unwrap();
        assert_eq!(part_defs.len(), 2);

        let instances = doc.instances.unwrap();
        assert_eq!(instances.len(), 2);

        let joints = doc.joints.unwrap();
        assert_eq!(joints.len(), 1);

        // Check joint type
        let joint = &joints[0];
        assert_eq!(joint.id, "base_to_arm");
        match &joint.kind {
            JointKind::Revolute {
                axis,
                limits,
                effort_limit,
                velocity_limit,
            } => {
                assert!((axis.z - 1.0).abs() < 0.01);
                assert!(limits.is_some());
                let (lower, upper) = limits.unwrap();
                // -1.57 rad ≈ -90 deg
                assert!((lower - (-90.0)).abs() < 1.0);
                assert!((upper - 90.0).abs() < 1.0);
                // effort passes through in N·m; velocity 1 rad/s → deg/s
                assert_eq!(*effort_limit, Some(10.0));
                assert!((velocity_limit.unwrap() - 1.0_f64.to_degrees()).abs() < 1e-9);
            }
            _ => panic!("Expected Revolute joint"),
        }
    }

    #[test]
    fn test_parse_actuator_limits_k1_knee() {
        // Booster K1 knee: 40 N·m effort, 12.5 rad/s velocity.
        let urdf = r#"<?xml version="1.0"?>
<robot name="k1_knee">
    <link name="thigh"/>
    <link name="shank"/>
    <joint name="knee" type="revolute">
        <parent link="thigh"/>
        <child link="shank"/>
        <axis xyz="0 1 0"/>
        <limit lower="-0.1" upper="2.27" effort="40" velocity="12.5"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        match &joints[0].kind {
            JointKind::Revolute {
                effort_limit,
                velocity_limit,
                ..
            } => {
                assert_eq!(*effort_limit, Some(40.0));
                assert!((velocity_limit.unwrap() - 12.5_f64.to_degrees()).abs() < 1e-9);
            }
            _ => panic!("Expected Revolute joint"),
        }
    }

    /// A humanoid-shaped URDF that, like Booster's K1 and most real ones,
    /// ships its world link + floating joint commented out.
    const COMMENTED_FLOATING: &str = r#"<?xml version="1.0"?>
<robot name="k1">
    <!-- <link name="world"/>
    <joint name="world_joint" type="floating">
      <origin xyz="0 0 0"/>
      <parent link="world"/>
      <child link="Trunk"/>
    </joint> -->
    <link name="Trunk"/>
    <link name="Thigh"/>
    <joint name="hip" type="revolute">
        <parent link="Trunk"/>
        <child link="Thigh"/>
        <axis xyz="0 1 0"/>
        <limit lower="-1" upper="1" effort="40" velocity="12.5"/>
    </joint>
</robot>"#;

    fn floating_opts(height_mm: f64) -> UrdfReadOptions {
        UrdfReadOptions {
            floating_base: true,
            spawn_height_mm: Some(height_mm),
            ..UrdfReadOptions::default()
        }
    }

    #[test]
    fn test_floating_base_off_by_default() {
        let doc = read_urdf_from_str(COMMENTED_FLOATING).unwrap();
        // Unchanged: Trunk is still the ground, no Free joint synthesized.
        let instances = doc.instances.as_ref().unwrap();
        assert_eq!(instances.len(), 2);
        assert_eq!(doc.ground_instance_id.as_deref(), Some("Trunk_inst"));
        let joints = doc.joints.as_ref().unwrap();
        assert_eq!(joints.len(), 1);
        assert!(!matches!(joints[0].kind, JointKind::Free));
    }

    #[test]
    fn test_floating_base_synthesizes_free_root_joint() {
        let doc =
            read_urdf_from_str_with_options(COMMENTED_FLOATING, &floating_opts(620.0)).unwrap();

        // world is now the grounded link, Trunk hangs off it via Free.
        assert_eq!(doc.ground_instance_id.as_deref(), Some("world_inst"));
        assert_eq!(doc.instances.as_ref().unwrap().len(), 3);

        let joints = doc.joints.as_ref().unwrap();
        let free = joints
            .iter()
            .find(|j| matches!(j.kind, JointKind::Free))
            .expect("synthesized floating joint must map to JointKind::Free");
        assert_eq!(free.parent_instance_id.as_deref(), Some("world_inst"));
        assert_eq!(free.child_instance_id, "Trunk_inst");
        // parentAnchor.z is what sets the spawn height (state is a scalar
        // and meaningless for 6 DOF).
        assert!((free.parent_anchor.z - 620.0).abs() < 1e-9);
        // The authored hip joint survives untouched.
        assert_eq!(joints.len(), 2);
    }

    #[test]
    fn test_floating_base_noop_when_urdf_already_has_one() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="declared">
    <link name="world"/>
    <link name="Trunk"/>
    <joint name="world_joint" type="floating">
        <origin xyz="0 0 1.0"/>
        <parent link="world"/>
        <child link="Trunk"/>
    </joint>
</robot>"#;
        // An explicit spawn height applies to the AUTHORED joint too. It used
        // to be discarded, which meant `--spawn-height-mm` silently did nothing
        // on exactly the URDFs it matters most for: the Booster K1's floating
        // variant authors `xyz="0 0 0"`, so the robot spawned at the world
        // origin, below its own termination floor, and every episode ended on
        // step 1 while the CLI reported the height it had ignored.
        let doc = read_urdf_from_str_with_options(urdf, &floating_opts(620.0)).unwrap();
        let joints = doc.joints.as_ref().unwrap();
        assert_eq!(joints.len(), 1, "must not add a second floating joint");
        assert!(
            (joints[0].parent_anchor.z - 620.0).abs() < 1e-9,
            "an explicit spawn height must win, got {}",
            joints[0].parent_anchor.z
        );

        // ...and with no height requested, the authored origin is preserved.
        let mut opts = floating_opts(0.0);
        opts.spawn_height_mm = None;
        let doc = read_urdf_from_str_with_options(urdf, &opts).unwrap();
        let joints = doc.joints.as_ref().unwrap();
        assert!(
            (joints[0].parent_anchor.z - 1000.0).abs() < 1e-9,
            "without an explicit height the URDF's own origin must survive"
        );
    }

    #[test]
    fn test_floating_base_explicit_link() {
        let opts = UrdfReadOptions {
            floating_base: true,
            floating_base_link: Some("Thigh".to_string()),
            ..UrdfReadOptions::default()
        };
        let doc = read_urdf_from_str_with_options(COMMENTED_FLOATING, &opts).unwrap();
        let free = doc
            .joints
            .as_ref()
            .unwrap()
            .iter()
            .find(|j| matches!(j.kind, JointKind::Free))
            .unwrap()
            .clone();
        assert_eq!(free.child_instance_id, "Thigh_inst");
    }

    #[test]
    fn test_floating_base_unknown_link_is_an_error() {
        let opts = UrdfReadOptions {
            floating_base: true,
            floating_base_link: Some("Nope".to_string()),
            ..UrdfReadOptions::default()
        };
        assert!(read_urdf_from_str_with_options(COMMENTED_FLOATING, &opts).is_err());
    }

    #[test]
    fn test_detect_commented_out_floating_joint() {
        assert_eq!(
            commented_out_floating_joint(COMMENTED_FLOATING).as_deref(),
            Some("world_joint")
        );
        // An *active* floating joint is not a comment hit.
        assert!(commented_out_floating_joint(
            r#"<robot name="a"><joint name="w" type="floating"/></robot>"#
        )
        .is_none());
        // Ordinary comments don't trip it.
        assert!(commented_out_floating_joint(
            r#"<robot name="a"><!-- a plain note --><link name="l"/></robot>"#
        )
        .is_none());
    }

    #[test]
    fn test_floating_base_name_collision() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="collide">
    <link name="world"/>
    <link name="Trunk"/>
    <joint name="world_joint" type="fixed">
        <parent link="world"/>
        <child link="Trunk"/>
    </joint>
</robot>"#;
        // `world`/`world_joint` are taken by a *fixed* joint here, so the
        // synthesized pair must not clash with them.
        let doc = read_urdf_from_str_with_options(urdf, &floating_opts(100.0)).unwrap();
        let instances = doc.instances.as_ref().unwrap();
        assert_eq!(instances.len(), 3);
        assert!(instances.iter().any(|i| i.id == "world_1_inst"));
        let joints = doc.joints.as_ref().unwrap();
        assert_eq!(
            joints
                .iter()
                .filter(|j| matches!(j.kind, JointKind::Free))
                .count(),
            1
        );
    }

    #[test]
    fn test_parse_continuous_joint() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="wheel">
    <link name="base"/>
    <link name="wheel"/>
    <joint name="wheel_joint" type="continuous">
        <parent link="base"/>
        <child link="wheel"/>
        <axis xyz="0 1 0"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        let joint = &joints[0];

        match &joint.kind {
            JointKind::Revolute { axis, limits, .. } => {
                assert!((axis.y - 1.0).abs() < 0.01);
                assert!(limits.is_none()); // Continuous has no limits
            }
            _ => panic!("Expected Revolute joint for continuous"),
        }
    }

    #[test]
    fn test_parse_floating_joint_maps_to_free() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="humanoid">
    <link name="base"/>
    <link name="pelvis"/>
    <joint name="world_joint" type="floating">
        <parent link="base"/>
        <child link="pelvis"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        assert!(
            matches!(joints[0].kind, JointKind::Free),
            "floating must map to Free (6-DOF), got {:?}",
            joints[0].kind
        );
    }

    #[test]
    fn test_parse_planar_joint_maps_to_ball() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="p">
    <link name="base"/>
    <link name="slider"/>
    <joint name="pj" type="planar">
        <parent link="base"/>
        <child link="slider"/>
        <axis xyz="0 0 1"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        // Documented approximation: vcad has no planar joint kind yet.
        assert!(matches!(joints[0].kind, JointKind::Ball));
    }

    #[test]
    fn test_parse_prismatic_joint() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="linear">
    <link name="base"/>
    <link name="slider"/>
    <joint name="slide_joint" type="prismatic">
        <parent link="base"/>
        <child link="slider"/>
        <axis xyz="1 0 0"/>
        <limit lower="0" upper="0.5" effort="100" velocity="0.5"/>
    </joint>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();
        let joints = doc.joints.unwrap();
        let joint = &joints[0];

        match &joint.kind {
            JointKind::Slider { axis, limits, .. } => {
                assert!((axis.x - 1.0).abs() < 0.01);
                assert!(limits.is_some());
                let (lower, upper) = limits.unwrap();
                // 0.5m = 500mm
                assert!((lower - 0.0).abs() < 0.1);
                assert!((upper - 500.0).abs() < 0.1);
            }
            _ => panic!("Expected Slider joint"),
        }
    }

    /// A cart-shaped link whose collision geometry is a convex decomposition,
    /// modelled on XLeRobot's `base_link` (3 `<visual>` / 8 `<collision>`) and
    /// `Moving_Jaw` (1 / 3). Visual and collision children interleave, which is
    /// what `normalize_robot_child_order` and quick-xml's contiguous-sibling
    /// rule have to survive. Primitives rather than meshes so the fixture needs
    /// no files on disk.
    const DECOMPOSED_URDF: &str = r#"<?xml version="1.0"?>
<robot name="cart">
    <link name="base_link">
        <visual><geometry><box size="0.30 0.20 0.05"/></geometry></visual>
        <collision>
            <origin xyz="-0.12 0 0"/>
            <geometry><box size="0.06 0.20 0.05"/></geometry>
        </collision>
        <visual>
            <origin xyz="0 0 0.06"/>
            <geometry><box size="0.10 0.10 0.08"/></geometry>
        </visual>
        <collision>
            <origin xyz="0.12 0 0"/>
            <geometry><box size="0.06 0.20 0.05"/></geometry>
        </collision>
        <collision>
            <origin xyz="0 -0.08 0"/>
            <geometry><box size="0.30 0.04 0.05"/></geometry>
        </collision>
        <collision>
            <origin xyz="0 0.08 0"/>
            <geometry><box size="0.30 0.04 0.05"/></geometry>
        </collision>
        <visual>
            <origin xyz="0 0 -0.03"/>
            <geometry><cylinder radius="0.04" length="0.02"/></geometry>
        </visual>
        <collision>
            <origin xyz="-0.12 -0.08 -0.04" rpy="1.5708 0 0"/>
            <geometry><cylinder radius="0.03" length="0.02"/></geometry>
        </collision>
        <collision>
            <origin xyz="0.12 -0.08 -0.04" rpy="1.5708 0 0"/>
            <geometry><cylinder radius="0.03" length="0.02"/></geometry>
        </collision>
        <collision>
            <origin xyz="-0.12 0.08 -0.04" rpy="1.5708 0 0"/>
            <geometry><cylinder radius="0.03" length="0.02"/></geometry>
        </collision>
        <collision>
            <origin xyz="0.12 0.08 -0.04" rpy="1.5708 0 0"/>
            <geometry><cylinder radius="0.03" length="0.02"/></geometry>
        </collision>
    </link>
    <link name="Moving_Jaw">
        <visual><geometry><box size="0.04 0.02 0.06"/></geometry></visual>
        <collision><geometry><box size="0.04 0.02 0.02"/></geometry></collision>
        <collision>
            <origin xyz="0 0 0.02"/>
            <geometry><box size="0.02 0.02 0.02"/></geometry>
        </collision>
        <collision>
            <origin xyz="0 0 0.04"/>
            <geometry><sphere radius="0.01"/></geometry>
        </collision>
    </link>
    <joint name="jaw" type="revolute">
        <parent link="base_link"/>
        <child link="Moving_Jaw"/>
        <origin xyz="0.15 0 0"/>
        <axis xyz="0 1 0"/>
        <limit lower="0" upper="1" effort="5" velocity="1"/>
    </joint>
</robot>"#;

    #[test]
    fn every_collision_element_becomes_its_own_collider_root() {
        let doc = read_urdf_from_str(DECOMPOSED_URDF).unwrap();
        let part_defs = doc.part_defs.as_ref().unwrap();

        let base = &part_defs["part_base_link"];
        let colliders = base
            .colliders
            .as_ref()
            .expect("a link with <collision> children must author colliders");
        assert_eq!(
            colliders.len(),
            8,
            "all 8 <collision> elements must survive as separate roots — collapsing \
             them to one throws away the convex decomposition"
        );
        let jaw = &part_defs["part_Moving_Jaw"];
        assert_eq!(jaw.colliders.as_ref().unwrap().len(), 3);

        // Every collider root is a real node, and they are all distinct.
        let unique: std::collections::HashSet<_> = colliders.iter().collect();
        assert_eq!(
            unique.len(),
            colliders.len(),
            "collider roots must be distinct"
        );
        for root in colliders {
            assert!(
                doc.nodes.contains_key(root),
                "collider root {root} is dangling"
            );
        }
    }

    #[test]
    fn visual_geometry_still_drives_the_rendered_root() {
        let doc = read_urdf_from_str(DECOMPOSED_URDF).unwrap();
        let part_defs = doc.part_defs.as_ref().unwrap();
        let base = &part_defs["part_base_link"];

        // The part's root is the *first visual* — the 300×200×50 plate — not
        // any collision piece. Collision geometry is for contact; the visual
        // is what renders.
        let root = &doc.nodes[&base.root];
        let size = match &root.op {
            CsgOp::Cube { size } => *size,
            // The first visual has no <origin>, so its root is the centering
            // Translate over the cube.
            CsgOp::Translate { child, .. } => match &doc.nodes[child].op {
                CsgOp::Cube { size } => *size,
                other => {
                    panic!("expected the visual box under the centering translate, got {other:?}")
                }
            },
            other => panic!("expected the visual box at the part root, got {other:?}"),
        };
        assert!((size.x - 300.0).abs() < 1e-6, "got {size:?}");
        assert!((size.y - 200.0).abs() < 1e-6, "got {size:?}");

        // None of the collider roots is the render root.
        assert!(!base.colliders.as_ref().unwrap().contains(&base.root));

        // Collider roots never render: `roots` carries one scene entry per
        // link, pointing at the part root.
        let scene_roots: std::collections::HashSet<_> = doc.roots.iter().map(|e| e.root).collect();
        for c in base.colliders.as_ref().unwrap() {
            assert!(
                !scene_roots.contains(c),
                "collider root {c} leaked into the scene and would render"
            );
        }
    }

    #[test]
    fn collision_origin_places_each_piece() {
        let doc = read_urdf_from_str(DECOMPOSED_URDF).unwrap();
        let part_defs = doc.part_defs.as_ref().unwrap();
        let colliders = part_defs["part_base_link"].colliders.clone().unwrap();

        // Piece 0: `<origin xyz="-0.12 0 0">` over a 60×200×50 box. The
        // subtree is Translate(origin) → Translate(centering) → Cube.
        let CsgOp::Translate { child, offset } = &doc.nodes[&colliders[0]].op else {
            panic!("expected the <origin> translate at the collider root");
        };
        assert!(
            (offset.x - -120.0).abs() < 1e-6,
            "URDF metres → mm: {offset:?}"
        );
        let CsgOp::Translate { child, offset } = &doc.nodes[child].op else {
            panic!("expected the primitive centering translate");
        };
        assert!(
            (offset.x - -30.0).abs() < 1e-6,
            "cube centering: {offset:?}"
        );
        assert!(matches!(doc.nodes[child].op, CsgOp::Cube { .. }));

        // Piece 4 carries an rpy, so it gains a Rotate between the two.
        let CsgOp::Translate { child, .. } = &doc.nodes[&colliders[4]].op else {
            panic!("expected the <origin> translate");
        };
        let CsgOp::Rotate { angles, .. } = &doc.nodes[child].op else {
            panic!("expected an rpy Rotate under the origin translate");
        };
        assert!(
            (angles.x - 90.0).abs() < 1e-3,
            "1.5708 rad → 90°, got {angles:?}"
        );
    }

    #[test]
    fn link_without_collisions_authors_no_colliders() {
        // The overwhelmingly common case, and the one every non-URDF authoring
        // path relies on: no `<collision>` means "the part is its own
        // collider", spelled as an absent list rather than a copy of `root`.
        let doc = read_urdf_from_str(SIMPLE_URDF).unwrap();
        for part in doc.part_defs.as_ref().unwrap().values() {
            assert!(part.colliders.is_none(), "{:?}", part.name);
        }
    }

    #[test]
    fn collision_only_link_reuses_its_first_piece_as_the_render_root() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="collision_only">
    <link name="hidden">
        <collision><geometry><box size="0.1 0.1 0.1"/></geometry></collision>
        <collision>
            <origin xyz="0.2 0 0"/>
            <geometry><box size="0.1 0.1 0.1"/></geometry>
        </collision>
    </link>
</robot>"#;
        let doc = read_urdf_from_str(urdf).unwrap();
        let part = &doc.part_defs.as_ref().unwrap()["part_hidden"];
        let colliders = part.colliders.as_ref().unwrap();
        assert_eq!(colliders.len(), 2);
        // With no <visual> to render, the root falls back to the first piece —
        // reusing that subtree rather than building a second copy of it.
        assert_eq!(part.root, colliders[0]);

        // That piece therefore *does* render, and should: collision geometry is
        // the only geometry the link has, so drawing nothing would be worse.
        // What must not happen is the other pieces leaking into the scene —
        // exactly one entry, pointing at the root.
        let scene_roots: Vec<_> = doc.roots.iter().map(|e| e.root).collect();
        assert_eq!(scene_roots, vec![part.root]);
        assert!(
            !scene_roots.contains(&colliders[1]),
            "collider piece 1 leaked into the scene and would render twice"
        );
    }

    #[test]
    fn test_geometry_conversion() {
        let urdf = r#"<?xml version="1.0"?>
<robot name="shapes">
    <link name="box">
        <visual>
            <geometry><box size="0.1 0.2 0.3"/></geometry>
        </visual>
    </link>
    <link name="cyl">
        <visual>
            <geometry><cylinder radius="0.05" length="0.2"/></geometry>
        </visual>
    </link>
    <link name="sph">
        <visual>
            <geometry><sphere radius="0.1"/></geometry>
        </visual>
    </link>
</robot>"#;

        let doc = read_urdf_from_str(urdf).unwrap();

        // Find the box node
        let box_node = doc
            .nodes
            .values()
            .find(|n| matches!(n.op, CsgOp::Cube { .. }))
            .unwrap();

        if let CsgOp::Cube { size } = &box_node.op {
            // 0.1m = 100mm
            assert!((size.x - 100.0).abs() < 0.1);
            assert!((size.y - 200.0).abs() < 0.1);
            assert!((size.z - 300.0).abs() < 0.1);
        }
    }
}

/// Write the requested spawn height onto the robot's floating joint.
///
/// Applies to an *authored* floating joint as well as a synthesized one. It
/// used to apply only to synthesized joints, on the reasoning that an authored
/// origin should win — but the practical effect was that importing a URDF whose
/// floating variant authors `xyz="0 0 0"` (the Booster K1 does) silently
/// discarded the requested height and dropped the robot at the world origin,
/// below its own termination floor, so every episode ended on step 1. A flag
/// named `spawn_height_mm` that does not set the spawn height is worse than no
/// flag. Leave it `None` to keep the authored origin.
fn apply_spawn_height(robot: &mut Robot, opts: &UrdfReadOptions) {
    let Some(mm) = opts.spawn_height_mm else {
        return;
    };
    let z_m = mm / 1000.0;
    for j in robot.joints.iter_mut() {
        if j.joint_type.trim() == "floating" {
            let rpy = j.origin.as_ref().and_then(|o| o.rpy.clone());
            j.origin = Some(crate::types::Origin {
                xyz: Some(format!("0 0 {z_m}")),
                rpy,
            });
        }
    }
}

/// `target` expressed relative to `base`, walking up with `..` as needed.
///
/// `std::path::Path` has no such operation (`strip_prefix` only handles the
/// case where `base` is an ancestor), and the common layout here — a document
/// in `examples/` referencing meshes in `third_party/` — is exactly the case
/// `strip_prefix` cannot express.
fn relative_path(target: &Path, base: &Path) -> Option<PathBuf> {
    let target = target.canonicalize().ok()?;
    let base = base.canonicalize().ok()?;
    let mut t = target.components().peekable();
    let mut b = base.components().peekable();
    while t.peek().is_some() && t.peek() == b.peek() {
        t.next();
        b.next();
    }
    let mut out = PathBuf::new();
    for _ in b {
        out.push("..");
    }
    for c in t {
        out.push(c);
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

#[cfg(test)]
mod relative_path_tests {
    use super::*;

    #[test]
    fn walks_up_and_back_down() {
        let tmp = std::env::temp_dir().join("vcad-relpath-test");
        let a = tmp.join("examples");
        let b = tmp.join("third_party/meshes");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let f = b.join("Trunk.STL");
        std::fs::write(&f, b"x").unwrap();

        let rel = relative_path(&f, &a).expect("a relative route exists");
        assert_eq!(
            rel,
            Path::new("../third_party/meshes/Trunk.STL"),
            "a sibling directory must be reached with `..`, which strip_prefix cannot do"
        );
        // And it must actually resolve back to the same file.
        assert!(a.join(&rel).canonicalize().unwrap() == f.canonicalize().unwrap());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
