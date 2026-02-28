/**
 * TypeScript mirror of the Rust `FeatureInput` discriminated union.
 *
 * Used with `WasmDocumentEngine.add_feature(JSON.stringify(input))` and
 * `WasmDocumentEngine.update_feature(id, JSON.stringify(input))`.
 */
export type FeatureInput =
  | { type: "Cube"; size_x: number; size_y: number; size_z: number }
  | { type: "Cylinder"; radius: number; height: number; segments?: number }
  | { type: "Sphere"; radius: number; segments?: number }
  | { type: "Cone"; radius_bottom: number; radius_top: number; height: number; segments?: number }
  | { type: "Boolean"; boolean_type: "union" | "difference" | "intersection"; input_a: string; input_b: string }
  | { type: "Extrude"; sketch: string; depth: number; direction: [number, number, number]; twist_angle?: number; scale_end?: number }
  | { type: "Revolve"; sketch: string; axis_origin: [number, number, number]; axis_dir: [number, number, number]; angle_deg: number }
  | { type: "Sweep"; sketch: string; path?: string; twist_angle?: number; scale_start?: number; scale_end?: number }
  | { type: "Loft"; profiles: string[]; closed?: boolean }
  | { type: "Fillet"; input: string; radius: number }
  | { type: "Chamfer"; input: string; distance: number }
  | { type: "Shell"; input: string; thickness: number }
  | { type: "LinearPattern"; input: string; direction: [number, number, number]; count: number; spacing: number }
  | { type: "CircularPattern"; input: string; axis_origin: [number, number, number]; axis_dir: [number, number, number]; count: number; angle_deg: number }
  | { type: "Mirror"; input: string; plane: string }
  | { type: "Text"; text: string; height: number; depth: number; alignment?: string; letter_spacing?: number; line_spacing?: number }
  | { type: "ImportedMesh"; positions_json: string; indices_json: string; normals_json?: string; source?: string }
  | { type: "PcbBoard"; board?: string }
  | { type: "EmbroideryPattern"; design?: string; source?: string }
  | { type: "PartDef"; source_feature: string; name?: string }
  | { type: "Instance"; part_def: string; name?: string; transform?: string }
  | { type: "Joint"; kind: string; child_instance: string; parent_instance?: string; anchor_a: [number, number, number]; anchor_b: [number, number, number]; axis?: [number, number, number]; name?: string; limits?: string }
  | { type: "SceneSettings"; environment?: string; lights?: string; background?: string; post_processing?: string; camera_presets?: string }
  | { type: "Schematic"; title?: string; sheet?: string };
