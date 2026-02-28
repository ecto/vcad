---
title: cad0 - Text to CAD
emoji: 🔧
colorFrom: green
colorTo: gray
sdk: gradio
sdk_version: 4.36.0
python_version: "3.10"
app_file: app.py
pinned: false
license: mit
hardware: zero-a10g
models:
  - campedersen/cad0
---

# cad0: Text to CAD

Generate parametric CAD geometry from natural language descriptions.

## How it works

1. Enter a description of a mechanical part (e.g., "L-bracket: 50mm x 30mm x 3mm thick")
2. The model generates VCode (a token-efficient DSL for CAD)
3. The IR is parsed and rendered to a 3D preview

## Model

- **cad0** is a fine-tuned Qwen2.5-Coder-7B trained on 530K synthetic CAD examples
- Outputs VCode which compiles to BRep (boundary representation) solids in [vcad](https://vcad.io)

## Links

- [vcad.io](https://vcad.io) - Web CAD app
- [Model on HuggingFace](https://huggingface.co/campedersen/cad0)
- [GitHub](https://github.com/ecto/vcad)
