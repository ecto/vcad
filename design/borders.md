# Borders

One rule for chrome borders across the web app:

- **Divider lines** (1px rules separating UI regions — top/bottom/left/right of a container) use `border-border/40`.
- **Box outlines** (borders that fully enclose an element: inputs, context menus, images, cards, buttons, swatches) use full `border-border`.

## Why

Mixing `/30`, `/40`, `/50`, and full opacity across adjacent chrome regions makes them visually "fight" — you notice the seams instead of the content. Picking one opacity for all divider lines lets the chrome recede. Box outlines stay full-opacity because they need to read as a contained element, not a seam.

## Applies to

Any horizontal/vertical rule around the shell:
- `AppShell` header/sidebar/footer edges
- `Header` menu-bar + tool-palette divider
- `StatusBar` top + internal dividers
- `LogViewer` top + tab/toolbar rows + row separators
- `ChatSidebar`, `PropertyPanel`, `FeatureTree`, `WhatsNewPanel` section dividers
- `MobileShell` header, bottom sheet, tool palette
- Shared primitives: `ui/panel.tsx` `PanelHeader`, `ui/dialog.tsx` header + footer

## Does not apply to

- `border border-border` on inputs, images, swatches, context-menu content — these are box outlines.
- Non-neutral divider colors (e.g. `border-danger/30`) keep their existing opacity.

When adding a new panel or chrome element, follow the rule. If a divider needs to be more prominent than /40 it's probably a box outline and should be full — not a different opacity.
