import type { ToolSchemaEntry } from "./types.js";

/** Static tool schemas generated from CsgOp. Regenerate with: cargo run --quiet --example dump_schemas -p vcad-ir > packages/core/src/commands/static-schemas.ts */
export const STATIC_TOOL_SCHEMAS: ToolSchemaEntry[] = [
    {
      "name": "cube",
      "description": "Axis-aligned box centered at origin.",
      "category": "primitive",
      "ai_hint": "Use for rectangular/box shapes. Size is width(x), depth(y), height(z).",
      "input_schema": {
        "properties": {
          "size": {
            "description": "Size along each axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          }
        },
        "required": [
          "size"
        ],
        "type": "object"
      }
    },
    {
      "name": "cylinder",
      "description": "Cylinder along the Z axis, centered at origin.",
      "category": "primitive",
      "ai_hint": "Axis along Z. Use for round shapes, pins, holes.",
      "input_schema": {
        "properties": {
          "height": {
            "description": "Height of the cylinder.",
            "type": "number"
          },
          "radius": {
            "description": "Radius of the cylinder.",
            "type": "number"
          },
          "segments": {
            "description": "Number of circular segments (0 = auto).",
            "type": "integer"
          }
        },
        "required": [
          "radius",
          "height",
          "segments"
        ],
        "type": "object"
      }
    },
    {
      "name": "sphere",
      "description": "Sphere centered at origin.",
      "category": "primitive",
      "input_schema": {
        "properties": {
          "radius": {
            "description": "Radius of the sphere.",
            "type": "number"
          },
          "segments": {
            "description": "Number of circular segments (0 = auto).",
            "type": "integer"
          }
        },
        "required": [
          "radius",
          "segments"
        ],
        "type": "object"
      }
    },
    {
      "name": "cone",
      "description": "Cone along the Z axis, centered at origin.",
      "category": "primitive",
      "input_schema": {
        "properties": {
          "height": {
            "description": "Height of the cone.",
            "type": "number"
          },
          "radius_bottom": {
            "description": "Bottom radius.",
            "type": "number"
          },
          "radius_top": {
            "description": "Top radius (0 for a point).",
            "type": "number"
          },
          "segments": {
            "description": "Number of circular segments (0 = auto).",
            "type": "integer"
          }
        },
        "required": [
          "radius_bottom",
          "radius_top",
          "height",
          "segments"
        ],
        "type": "object"
      }
    },
    {
      "name": "union",
      "description": "Boolean union of two geometries.",
      "category": "boolean",
      "input_schema": {
        "properties": {
          "left": {
            "description": "Left operand.",
            "type": "string"
          },
          "right": {
            "description": "Right operand.",
            "type": "string"
          }
        },
        "required": [
          "left",
          "right"
        ],
        "type": "object"
      }
    },
    {
      "name": "difference",
      "description": "Boolean difference (left minus right).",
      "category": "boolean",
      "input_schema": {
        "properties": {
          "left": {
            "description": "Left operand (base).",
            "type": "string"
          },
          "right": {
            "description": "Right operand (subtracted).",
            "type": "string"
          }
        },
        "required": [
          "left",
          "right"
        ],
        "type": "object"
      }
    },
    {
      "name": "intersection",
      "description": "Boolean intersection of two geometries.",
      "category": "boolean",
      "input_schema": {
        "properties": {
          "left": {
            "description": "Left operand.",
            "type": "string"
          },
          "right": {
            "description": "Right operand.",
            "type": "string"
          }
        },
        "required": [
          "left",
          "right"
        ],
        "type": "object"
      }
    },
    {
      "name": "translate",
      "description": "Translation by an offset vector.",
      "category": "transform",
      "input_schema": {
        "properties": {
          "child": {
            "description": "Child node to translate.",
            "type": "string"
          },
          "offset": {
            "description": "Translation offset.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          }
        },
        "required": [
          "child",
          "offset"
        ],
        "type": "object"
      }
    },
    {
      "name": "rotate",
      "description": "Rotation by Euler angles in degrees (applied as X, then Y, then Z).",
      "category": "transform",
      "input_schema": {
        "properties": {
          "angles": {
            "description": "Rotation angles in degrees.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "child": {
            "description": "Child node to rotate.",
            "type": "string"
          }
        },
        "required": [
          "child",
          "angles"
        ],
        "type": "object"
      }
    },
    {
      "name": "scale",
      "description": "Non-uniform scale.",
      "category": "transform",
      "input_schema": {
        "properties": {
          "child": {
            "description": "Child node to scale.",
            "type": "string"
          },
          "factor": {
            "description": "Scale factors per axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          }
        },
        "required": [
          "child",
          "factor"
        ],
        "type": "object"
      }
    },
    {
      "name": "sketch_2_d",
      "description": "A 2D sketch profile on a plane.  The sketch defines a closed profile in a local 2D coordinate system. Use with [`CsgOp::Extrude`] or [`CsgOp::Revolve`] to create 3D geometry.",
      "category": "sketch_op",
      "ai_hint": "Defines a closed 2D profile. Segments are Line{start,end} or Arc{start,end,center,ccw}. Usually used inline with extrude/revolve — prefer creating extrude directly with inline sketch.",
      "input_schema": {
        "properties": {
          "origin": {
            "description": "Origin point of the sketch plane in 3D.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "segments": {
            "description": "The segments forming the closed profile.",
            "items": {
              "oneOf": [
                {
                  "description": "Straight line segment from start to end.",
                  "properties": {
                    "end": {
                      "properties": {
                        "x": {
                          "type": "number"
                        },
                        "y": {
                          "type": "number"
                        }
                      },
                      "required": [
                        "x",
                        "y"
                      ],
                      "type": "object"
                    },
                    "start": {
                      "properties": {
                        "x": {
                          "type": "number"
                        },
                        "y": {
                          "type": "number"
                        }
                      },
                      "required": [
                        "x",
                        "y"
                      ],
                      "type": "object"
                    },
                    "type": {
                      "const": "Line"
                    }
                  },
                  "required": [
                    "type",
                    "start",
                    "end"
                  ],
                  "type": "object"
                },
                {
                  "description": "Circular arc. Radius is implied — distance(start,center) must equal distance(end,center).",
                  "properties": {
                    "ccw": {
                      "description": "True = counter-clockwise from start to end.",
                      "type": "boolean"
                    },
                    "center": {
                      "properties": {
                        "x": {
                          "type": "number"
                        },
                        "y": {
                          "type": "number"
                        }
                      },
                      "required": [
                        "x",
                        "y"
                      ],
                      "type": "object"
                    },
                    "end": {
                      "properties": {
                        "x": {
                          "type": "number"
                        },
                        "y": {
                          "type": "number"
                        }
                      },
                      "required": [
                        "x",
                        "y"
                      ],
                      "type": "object"
                    },
                    "start": {
                      "properties": {
                        "x": {
                          "type": "number"
                        },
                        "y": {
                          "type": "number"
                        }
                      },
                      "required": [
                        "x",
                        "y"
                      ],
                      "type": "object"
                    },
                    "type": {
                      "const": "Arc"
                    }
                  },
                  "required": [
                    "type",
                    "start",
                    "end",
                    "center",
                    "ccw"
                  ],
                  "type": "object"
                }
              ]
            },
            "type": "array"
          },
          "x_dir": {
            "description": "Unit vector along the local X axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "y_dir": {
            "description": "Unit vector along the local Y axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          }
        },
        "required": [
          "origin",
          "x_dir",
          "y_dir",
          "segments"
        ],
        "type": "object"
      }
    },
    {
      "name": "extrude",
      "description": "Extrude a sketch profile along a direction vector.",
      "category": "sketch_op",
      "ai_hint": "PREFERRED for custom shapes. Pass sketch as inline object with origin, x_dir, y_dir, segments. Example: {sketch: {origin:{x:0,y:0,z:0}, x_dir:{x:1,y:0,z:0}, y_dir:{x:0,y:1,z:0}, segments:[{type:'Line',start:{x:0,y:0},end:{x:20,y:0}},{type:'Line',start:{x:20,y:0},end:{x:20,y:15}},{type:'Line',start:{x:20,y:15},end:{x:0,y:15}},{type:'Line',start:{x:0,y:15},end:{x:0,y:0}}]}, direction:{x:0,y:0,z:10}}",
      "input_schema": {
        "properties": {
          "direction": {
            "description": "Extrusion direction and distance (length of vector = extrusion depth).",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "scale_end": {
            "description": "Optional scale factor at end of extrusion (1.0 = no taper).",
            "type": "number"
          },
          "sketch": {
            "description": "The sketch node to extrude.",
            "type": "string"
          },
          "twist_angle": {
            "description": "Optional twist angle in radians (rotation around extrusion axis).",
            "type": "number"
          }
        },
        "required": [
          "sketch",
          "direction"
        ],
        "type": "object"
      }
    },
    {
      "name": "revolve",
      "description": "Revolve a sketch profile around an axis.",
      "category": "sketch_op",
      "input_schema": {
        "properties": {
          "angle_deg": {
            "description": "Revolution angle in degrees (360 for full revolution).",
            "type": "number"
          },
          "axis_dir": {
            "description": "Direction of the revolution axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "axis_origin": {
            "description": "A point on the revolution axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "sketch": {
            "description": "The sketch node to revolve.",
            "type": "string"
          }
        },
        "required": [
          "sketch",
          "axis_origin",
          "axis_dir",
          "angle_deg"
        ],
        "type": "object"
      }
    },
    {
      "name": "linear_pattern",
      "description": "Linear pattern — repeat geometry along a direction.",
      "category": "pattern",
      "input_schema": {
        "properties": {
          "child": {
            "description": "Child node to pattern.",
            "type": "string"
          },
          "count": {
            "description": "Number of copies (including original).",
            "type": "integer"
          },
          "direction": {
            "description": "Direction vector (will be normalized).",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "spacing": {
            "description": "Spacing between copies along direction.",
            "type": "number"
          }
        },
        "required": [
          "child",
          "direction",
          "count",
          "spacing"
        ],
        "type": "object"
      }
    },
    {
      "name": "circular_pattern",
      "description": "Circular pattern — repeat geometry around an axis.",
      "category": "pattern",
      "input_schema": {
        "properties": {
          "angle_deg": {
            "description": "Total angle span in degrees.",
            "type": "number"
          },
          "axis_dir": {
            "description": "Direction of the rotation axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "axis_origin": {
            "description": "A point on the rotation axis.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "child": {
            "description": "Child node to pattern.",
            "type": "string"
          },
          "count": {
            "description": "Number of copies (including original).",
            "type": "integer"
          }
        },
        "required": [
          "child",
          "axis_origin",
          "axis_dir",
          "count",
          "angle_deg"
        ],
        "type": "object"
      }
    },
    {
      "name": "shell",
      "description": "Shell — hollow out a solid by offsetting faces.",
      "category": "modifier",
      "ai_hint": "Hollow out a solid. Use parent_part_id. Great for enclosures, cups, containers.",
      "input_schema": {
        "properties": {
          "child": {
            "description": "Child node to shell.",
            "type": "string"
          },
          "thickness": {
            "description": "Wall thickness (inward offset).",
            "type": "number"
          }
        },
        "required": [
          "child",
          "thickness"
        ],
        "type": "object"
      }
    },
    {
      "name": "fillet",
      "description": "Fillet — round edges of a solid.",
      "category": "modifier",
      "ai_hint": "Apply after creating geometry. Use parent_part_id to target a part. Typical radius: 1-5mm for small features, 5-20mm for large.",
      "input_schema": {
        "properties": {
          "child": {
            "description": "Child node to fillet.",
            "type": "string"
          },
          "radius": {
            "description": "Fillet radius.",
            "type": "number"
          }
        },
        "required": [
          "child",
          "radius"
        ],
        "type": "object"
      }
    },
    {
      "name": "chamfer",
      "description": "Chamfer — bevel edges of a solid.",
      "category": "modifier",
      "ai_hint": "Bevel edges. Apply after creating geometry. Use parent_part_id to target a part.",
      "input_schema": {
        "properties": {
          "child": {
            "description": "Child node to chamfer.",
            "type": "string"
          },
          "distance": {
            "description": "Chamfer distance.",
            "type": "number"
          }
        },
        "required": [
          "child",
          "distance"
        ],
        "type": "object"
      }
    },
    {
      "name": "text_2_d",
      "description": "2D text that can be extruded into 3D geometry.  Creates sketch profiles from text glyphs, which can then be extruded and used in boolean operations for embossing/engraving.",
      "category": "sketch_op",
      "input_schema": {
        "properties": {
          "alignment": {
            "description": "Text alignment.",
            "type": "object"
          },
          "font": {
            "description": "Font name (e.g., \"sans-serif\", \"monospace\", or custom registered font).",
            "type": "string"
          },
          "height": {
            "description": "Text height in mm.",
            "type": "number"
          },
          "letter_spacing": {
            "description": "Letter spacing multiplier (1.0 = normal).",
            "type": "number"
          },
          "line_spacing": {
            "description": "Line spacing multiplier for multi-line text (1.0 = normal).",
            "type": "number"
          },
          "origin": {
            "description": "Origin point of the text plane in 3D.",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "text": {
            "description": "The text string to render.",
            "type": "string"
          },
          "x_dir": {
            "description": "X direction of the text plane (text flows along this axis).",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          },
          "y_dir": {
            "description": "Y direction of the text plane (text height along this axis).",
            "properties": {
              "x": {
                "type": "number"
              },
              "y": {
                "type": "number"
              },
              "z": {
                "type": "number"
              }
            },
            "required": [
              "x",
              "y",
              "z"
            ],
            "type": "object"
          }
        },
        "required": [
          "origin",
          "x_dir",
          "y_dir",
          "text",
          "font",
          "height",
          "alignment"
        ],
        "type": "object"
      }
    },
    {
      "name": "sweep",
      "description": "Sweep a profile along a path curve.",
      "category": "sketch_op",
      "ai_hint": "Sweep a 2D sketch profile along a 3D path. The `path` field is a tagged object — one of: `{type:'Line', start:{x,y,z}, end:{x,y,z}}` for a straight sweep, or `{type:'Helix', radius, pitch, height, turns}` for a coil/spring (radius = helix radius in mm, pitch = vertical rise per turn in mm, height = total Z height, turns = total turn count ≈ height/pitch). USE HELIX for springs, coils, screw threads, spiral ramps — do NOT try to approximate a helix with Line segments; a Line path only produces a straight sweep. Example spring: sketch a small circle profile, then sweep with path:{type:'Helix', radius:10, pitch:8, height:50, turns:6.25}.",
      "input_schema": {
        "properties": {
          "arc_segments": {
            "description": "Segments per arc in profile (default 8).",
            "type": "integer"
          },
          "orientation": {
            "description": "Initial profile rotation around path tangent (radians, default 0).",
            "type": "number"
          },
          "path": {
            "description": "The path curve to sweep along.",
            "oneOf": [
              {
                "description": "Straight line path. Use for linear extrudes-along-a-path.",
                "properties": {
                  "end": {
                    "properties": {
                      "x": {
                        "type": "number"
                      },
                      "y": {
                        "type": "number"
                      },
                      "z": {
                        "type": "number"
                      }
                    },
                    "required": [
                      "x",
                      "y",
                      "z"
                    ],
                    "type": "object"
                  },
                  "start": {
                    "properties": {
                      "x": {
                        "type": "number"
                      },
                      "y": {
                        "type": "number"
                      },
                      "z": {
                        "type": "number"
                      }
                    },
                    "required": [
                      "x",
                      "y",
                      "z"
                    ],
                    "type": "object"
                  },
                  "type": {
                    "const": "Line"
                  }
                },
                "required": [
                  "type",
                  "start",
                  "end"
                ],
                "type": "object"
              },
              {
                "description": "Helical path. Use for springs, coils, screw threads, spiral ramps.",
                "properties": {
                  "height": {
                    "description": "Total Z height of the helix in mm.",
                    "type": "number"
                  },
                  "pitch": {
                    "description": "Vertical rise per full turn in mm.",
                    "type": "number"
                  },
                  "radius": {
                    "description": "Helix radius in mm.",
                    "type": "number"
                  },
                  "turns": {
                    "description": "Total turn count. Normally ≈ height / pitch.",
                    "type": "number"
                  },
                  "type": {
                    "const": "Helix"
                  }
                },
                "required": [
                  "type",
                  "radius",
                  "pitch",
                  "height",
                  "turns"
                ],
                "type": "object"
              }
            ]
          },
          "path_segments": {
            "description": "Segments along path (0 = auto).",
            "type": "integer"
          },
          "scale_end": {
            "description": "Scale at end (default 1.0).",
            "type": "number"
          },
          "scale_start": {
            "description": "Scale at start (default 1.0).",
            "type": "number"
          },
          "sketch": {
            "description": "The sketch node to sweep.",
            "type": "string"
          },
          "twist_angle": {
            "description": "Total twist in radians (default 0).",
            "type": "number"
          }
        },
        "required": [
          "sketch",
          "path"
        ],
        "type": "object"
      }
    },
    {
      "name": "loft",
      "description": "Loft between multiple profiles.",
      "category": "sketch_op",
      "input_schema": {
        "properties": {
          "closed": {
            "description": "Connect last profile to first (creates tube).",
            "type": "boolean"
          },
          "sketches": {
            "description": "Array of Sketch2D node references (>= 2).",
            "items": {
              "description": "Node ID reference",
              "type": "string"
            },
            "type": "array"
          }
        },
        "required": [
          "sketches"
        ],
        "type": "object"
      }
    }
  ];
