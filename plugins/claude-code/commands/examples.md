---
name: examples
description: Show vcad pattern library with ready-to-use code snippets
---

Show the user a library of common CAD patterns they can use as starting points. Present them as a menu, then create whichever one they pick (or let them customize parameters).

## Pattern Library

### 1. Mounting Plate
Flat plate with corner mounting holes. Good for brackets, panels, and PCB mounts.
```json
{
  "parts": [{
    "name": "mounting_plate",
    "primitive": {"type": "cube", "size": {"x": 100, "y": 60, "z": 5}},
    "operations": [
      {"type": "hole", "diameter": 4, "at": {"x": 10, "y": 10}},
      {"type": "hole", "diameter": 4, "at": {"x": 90, "y": 10}},
      {"type": "hole", "diameter": 4, "at": {"x": 10, "y": 50}},
      {"type": "hole", "diameter": 4, "at": {"x": 90, "y": 50}}
    ]
  }]
}
```

### 2. L-Bracket
Right-angle bracket with mounting holes on both faces.
```json
{
  "parts": [{
    "name": "bracket",
    "primitive": {"type": "cube", "size": {"x": 40, "y": 40, "z": 5}},
    "operations": [
      {"type": "union", "primitive": {"type": "cube", "size": {"x": 5, "y": 40, "z": 30}}, "at": {"x": 0, "y": 0, "z": 5}},
      {"type": "hole", "diameter": 5, "at": {"x": 20, "y": 20}},
      {"type": "difference", "primitive": {"type": "cylinder", "radius": 2.5, "height": 40}, "at": {"x": 2.5, "y": 20, "z": 20}}
    ]
  }]
}
```

### 3. Flange
Cylinder with center bore and circular bolt pattern.
```json
{
  "parts": [{
    "name": "flange",
    "primitive": {"type": "cylinder", "radius": 40, "height": 10},
    "operations": [
      {"type": "hole", "diameter": 20, "at": "center"},
      {"type": "difference", "primitive": {"type": "cylinder", "radius": 4, "height": 15}, "at": {"x": 30, "y": 0, "z": -2}},
      {"type": "circular_pattern", "axis_origin": {"x": 0, "y": 0, "z": 0}, "axis_dir": {"x": 0, "y": 0, "z": 1}, "count": 6, "angle_deg": 360}
    ]
  }]
}
```

### 4. Enclosure
Hollow box created with the shell operation. Good for electronics housings.
```json
{
  "parts": [{
    "name": "enclosure",
    "primitive": {"type": "cube", "size": {"x": 80, "y": 60, "z": 40}},
    "operations": [
      {"type": "shell", "thickness": 2}
    ]
  }]
}
```

### 5. Bushing
Cylinder with a blind hole. Common for bearings and spacers.
```json
{
  "parts": [{
    "name": "bushing",
    "primitive": {"type": "cylinder", "radius": 15, "height": 20},
    "operations": [
      {"type": "hole", "diameter": 10, "depth": 15, "at": "center"}
    ]
  }]
}
```

### 6. Spring (Helix Sweep)
Wire swept along a helical path.
```json
{
  "parts": [{
    "name": "spring",
    "sweep": {
      "sketch": {"shape": {"type": "circle", "radius": 1.5}},
      "path": {"type": "helix", "radius": 10, "pitch": 8, "height": 40}
    },
    "material": "steel"
  }]
}
```

### 7. Rail with Linear Pattern
Extruded bar with evenly-spaced holes.
```json
{
  "parts": [{
    "name": "rail",
    "primitive": {"type": "cube", "size": {"x": 200, "y": 20, "z": 10}},
    "operations": [
      {"type": "difference", "primitive": {"type": "cylinder", "radius": 3, "height": 15}, "at": {"x": 20, "y": 10, "z": -2}},
      {"type": "linear_pattern", "direction": {"x": 1, "y": 0, "z": 0}, "count": 5, "spacing": 40}
    ]
  }]
}
```

Present these as a numbered list. When the user picks one, create it with `create_cad_document`, then run `inspect_cad` to show properties. Offer to customize dimensions, add features, or export.
