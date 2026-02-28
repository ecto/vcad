---
title: vcad Render API
emoji: 🔧
colorFrom: green
colorTo: blue
sdk: docker
app_port: 7860
pinned: false
---

# vcad Render API

HTTP API that renders [VCode](https://campedersen.com/cad0) to PNG images using the vcad kernel.

## Endpoints

### POST /render

Renders VCode to a PNG image.

**Request:**
```json
{
  "ir": "C 50 30 10\nY 5 20\nT 1 25 15 0\nD 0 2"
}
```

**Response:** PNG image (image/png)

### GET /health

Health check endpoint.

**Response:**
```json
{"status": "ok"}
```

## VCode Syntax

- `C w h d` - Box (cuboid)
- `Y r h` - Cylinder
- `S r` - Sphere
- `T idx x y z` - Translate node at index
- `U a b` - Union of nodes a and b
- `D a b` - Difference (a minus b)

## Related

- [cad0 model](https://huggingface.co/campedersen/cad0) - Text to VCode
- [vcad](https://vcad.io) - Full CAD app
