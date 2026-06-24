import AppKit

// The material palette, ported from the web app's `data/materials.ts` (31+
// real-world PBR presets). A part's render color resolves in this order:
// the document's own `materials` map → a preset by key → an index fallback.
// Assigning a preset writes its definition into the doc so it persists + renders
// identically on reload.

struct MaterialPreset: Identifiable {
    let key: String
    let category: String
    let color: (Double, Double, Double)
    let metallic: Float
    let roughness: Float
    let transmission: Float
    var id: String { key }

    var name: String {
        key.split(separator: "-").map { $0.prefix(1).uppercased() + $0.dropFirst() }.joined(separator: " ")
    }
    var nsColor: NSColor { NSColor(srgbRed: color.0, green: color.1, blue: color.2, alpha: 1) }

    static func byKey(_ k: String) -> MaterialPreset? { all.first { $0.key == k } }

    static let categoryOrder = ["metals", "plastics", "organic", "glass", "composite", "other"]
    /// Presets grouped by category in display order.
    static var grouped: [(category: String, items: [MaterialPreset])] {
        categoryOrder.compactMap { cat in
            let items = all.filter { $0.category == cat }
            return items.isEmpty ? nil : (cat, items)
        }
    }

    static let all: [MaterialPreset] = [
        .init(key: "aluminum", category: "metals", color: (0.8, 0.8, 0.85), metallic: 0.9, roughness: 0.3, transmission: 0),
        .init(key: "steel", category: "metals", color: (0.7, 0.7, 0.72), metallic: 0.95, roughness: 0.25, transmission: 0),
        .init(key: "brass", category: "metals", color: (0.85, 0.65, 0.3), metallic: 0.9, roughness: 0.35, transmission: 0),
        .init(key: "copper", category: "metals", color: (0.95, 0.5, 0.35), metallic: 0.95, roughness: 0.25, transmission: 0),
        .init(key: "titanium", category: "metals", color: (0.6, 0.6, 0.65), metallic: 0.85, roughness: 0.4, transmission: 0),
        .init(key: "chrome", category: "metals", color: (0.9, 0.9, 0.92), metallic: 1.0, roughness: 0.05, transmission: 0),
        .init(key: "gold", category: "metals", color: (1.0, 0.84, 0.0), metallic: 1.0, roughness: 0.1, transmission: 0),
        .init(key: "silver", category: "metals", color: (0.95, 0.95, 0.97), metallic: 1.0, roughness: 0.1, transmission: 0),
        .init(key: "abs-white", category: "plastics", color: (0.95, 0.95, 0.93), metallic: 0, roughness: 0.5, transmission: 0),
        .init(key: "abs-black", category: "plastics", color: (0.1, 0.1, 0.1), metallic: 0, roughness: 0.5, transmission: 0),
        .init(key: "abs-red", category: "plastics", color: (0.85, 0.15, 0.15), metallic: 0, roughness: 0.5, transmission: 0),
        .init(key: "abs-blue", category: "plastics", color: (0.2, 0.4, 0.85), metallic: 0, roughness: 0.5, transmission: 0),
        .init(key: "pla", category: "plastics", color: (0.85, 0.85, 0.8), metallic: 0, roughness: 0.45, transmission: 0),
        .init(key: "petg", category: "plastics", color: (0.75, 0.85, 0.9), metallic: 0, roughness: 0.35, transmission: 0),
        .init(key: "nylon", category: "plastics", color: (0.92, 0.9, 0.85), metallic: 0, roughness: 0.55, transmission: 0),
        .init(key: "resin", category: "plastics", color: (0.6, 0.55, 0.5), metallic: 0, roughness: 0.2, transmission: 0),
        .init(key: "acrylic", category: "plastics", color: (0.95, 0.95, 0.98), metallic: 0, roughness: 0.1, transmission: 0),
        .init(key: "rubber", category: "plastics", color: (0.15, 0.15, 0.15), metallic: 0, roughness: 0.8, transmission: 0),
        .init(key: "oak", category: "organic", color: (0.65, 0.5, 0.35), metallic: 0, roughness: 0.7, transmission: 0),
        .init(key: "walnut", category: "organic", color: (0.4, 0.28, 0.2), metallic: 0, roughness: 0.65, transmission: 0),
        .init(key: "leather", category: "organic", color: (0.45, 0.3, 0.2), metallic: 0, roughness: 0.75, transmission: 0),
        .init(key: "cork", category: "organic", color: (0.75, 0.6, 0.45), metallic: 0, roughness: 0.9, transmission: 0),
        .init(key: "bamboo", category: "organic", color: (0.85, 0.75, 0.55), metallic: 0, roughness: 0.6, transmission: 0),
        .init(key: "glass", category: "glass", color: (0.95, 0.97, 1.0), metallic: 0, roughness: 0.02, transmission: 1.0),
        .init(key: "glass-tinted", category: "glass", color: (0.85, 0.9, 0.95), metallic: 0, roughness: 0.05, transmission: 0.95),
        .init(key: "acrylic-clear", category: "glass", color: (0.98, 0.98, 1.0), metallic: 0, roughness: 0.05, transmission: 0.95),
        .init(key: "polycarbonate-frosted", category: "glass", color: (0.92, 0.94, 0.96), metallic: 0, roughness: 0.35, transmission: 0.7),
        .init(key: "carbon-fiber", category: "composite", color: (0.15, 0.15, 0.18), metallic: 0.3, roughness: 0.3, transmission: 0),
        .init(key: "fiberglass", category: "composite", color: (0.85, 0.85, 0.75), metallic: 0, roughness: 0.4, transmission: 0),
        .init(key: "kevlar", category: "composite", color: (0.75, 0.7, 0.3), metallic: 0, roughness: 0.6, transmission: 0),
        .init(key: "concrete", category: "other", color: (0.6, 0.6, 0.58), metallic: 0, roughness: 0.85, transmission: 0),
        .init(key: "ceramic", category: "other", color: (0.95, 0.93, 0.9), metallic: 0, roughness: 0.25, transmission: 0),
        .init(key: "foam", category: "other", color: (0.3, 0.3, 0.35), metallic: 0, roughness: 0.95, transmission: 0),
    ]
}

/// A part's render material, resolved from the doc's material def or a preset.
struct ResolvedMaterial {
    var color: NSColor
    var metallic: Float
    var roughness: Float
    var transmission: Float
}
