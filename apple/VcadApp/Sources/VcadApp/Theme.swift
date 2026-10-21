import SwiftUI

// The one design system. Every floating surface, radius, spacing constant,
// section eyebrow, key/value row and selection highlight in the app comes from
// here — so a panel in Manufacture cannot drift from a panel in Design, and a
// light-mode or wallpaper backdrop never meets a hard-coded dark-mode alpha.
//
// Type is semantic (`.body`, `.callout`, `.subheadline`, `.caption`) rather
// than point sizes: on macOS those resolve to 13 / 12 / 11 / 10 pt, track the
// user's text-size setting, and stay legible under accessibility scaling.

enum Theme {
    enum Radius {
        /// Floating panels and the workspace header.
        static let panel: CGFloat = 12
        /// Buttons, scrub fields, keycaps, badges.
        static let control: CGFloat = 6
        /// List rows (feature tree, operations, parts).
        static let row: CGFloat = 6
    }
    enum Space {
        static let xs: CGFloat = 4
        static let s: CGFloat = 8
        static let m: CGFloat = 12
        static let l: CGFloat = 16
    }
    enum Width {
        /// The left rail: model navigator, CNC job outline, circuit navigator.
        static let navigator: CGFloat = 236
        /// The right rail: object inspector, machine panel, circuit inspector.
        static let inspector: CGFloat = 268
        /// Manufacture's compact job outline.
        static let cncOutline: CGFloat = 192
    }
    enum Height {
        static let header: CGFloat = 52
    }
}

// MARK: - Surfaces

/// The floating panel surface: Liquid Glass where the OS has it, a regular
/// material with a hairline and a soft shadow everywhere else. The shadow
/// weight follows the appearance so a panel over a light desktop does not
/// carry a night-time shadow.
private struct PanelSurface<S: InsettableShape>: ViewModifier {
    let shape: S
    @Environment(\.colorScheme) private var scheme

    func body(content: Content) -> some View {
        #if os(macOS)
        if #available(macOS 26.0, *) {
            content.glassEffect(.regular, in: shape)
        } else {
            fallback(content)
        }
        #else
        fallback(content)
        #endif
    }

    private func fallback(_ content: Content) -> some View {
        content
            .background(.regularMaterial, in: shape)
            .overlay(shape.strokeBorder(.separator, lineWidth: 1))
            .shadow(color: .black.opacity(scheme == .dark ? 0.32 : 0.14), radius: 14, y: 5)
    }
}

extension View {
    /// A floating panel over the viewport.
    func panelSurface() -> some View {
        modifier(PanelSurface(shape: RoundedRectangle(cornerRadius: Theme.Radius.panel, style: .continuous)))
    }
    /// A floating pill (transports, hint bars, status chips).
    func pillSurface() -> some View {
        modifier(PanelSurface(shape: Capsule(style: .continuous)))
    }
    /// The background of a list row: accent selection, a quiet hover, or nothing.
    func selectableRow(selected: Bool, hovered: Bool = false) -> some View {
        background(
            selected ? AnyShapeStyle(.selection) : hovered ? AnyShapeStyle(.quaternary) : AnyShapeStyle(.clear),
            in: RoundedRectangle(cornerRadius: Theme.Radius.row, style: .continuous))
    }
    /// The inset well behind an editable value (scrub field, rename field).
    func controlWell(active: Bool = false) -> some View {
        background(.quaternary.opacity(active ? 1 : 0.6),
                   in: RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous))
        .overlay(RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
            .strokeBorder(active ? AnyShapeStyle(Color.accentColor.opacity(0.7)) : AnyShapeStyle(.separator),
                          lineWidth: active ? 1 : 0.5))
    }
}

// MARK: - Panel chrome

/// The title row of a floating panel: an optional symbol, the title, and the
/// one close affordance every panel shares.
struct PanelHeader: View {
    let title: String
    var systemImage: String? = nil
    var onClose: (() -> Void)? = nil

    var body: some View {
        HStack(spacing: Theme.Space.s) {
            if let systemImage {
                Image(systemName: systemImage).foregroundStyle(.secondary)
            }
            Text(title).font(.headline).lineLimit(1).truncationMode(.middle)
            Spacer(minLength: 0)
            if let onClose {
                Button(action: onClose) {
                    Image(systemName: "xmark.circle.fill").foregroundStyle(.tertiary)
                }
                .buttonStyle(.borderless)
                .help("Close \(title)")
                .accessibilityLabel("Close \(title)")
            }
        }
    }
}

/// A section eyebrow: small caps, tracked, secondary.
struct Eyebrow: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text.uppercased())
            .font(.caption2.weight(.semibold))
            .tracking(0.6)
            .foregroundStyle(.secondary)
            .accessibilityAddTraits(.isHeader)
    }
}

/// A read-only key/value row. The value is monospaced-digit so columns of
/// numbers line up; the key is secondary so the value reads first.
struct KeyValueRow: View {
    let key: String
    let value: String
    init(_ key: String, _ value: String) { self.key = key; self.value = value }
    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(key).foregroundStyle(.secondary)
            Spacer(minLength: Theme.Space.s)
            Text(value).monospacedDigit().multilineTextAlignment(.trailing)
                .textSelection(.enabled)
        }
        .font(.callout)
        .accessibilityElement(children: .combine)
    }
}

/// The empty state every panel uses.
struct EmptyPanelState: View {
    let title: String
    let systemImage: String
    var detail: String? = nil
    var body: some View {
        VStack(spacing: Theme.Space.xs) {
            Image(systemName: systemImage).font(.title2).foregroundStyle(.tertiary)
            Text(title).font(.callout).foregroundStyle(.secondary)
            if let detail {
                Text(detail).font(.caption).foregroundStyle(.tertiary)
                    .multilineTextAlignment(.center)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, Theme.Space.l)
    }
}

/// A count with its noun, pluralised: "1 feature", "3 features".
func counted(_ n: Int, _ noun: String, plural: String? = nil) -> String {
    "\(n) \(n == 1 ? noun : (plural ?? noun + "s"))"
}

// MARK: - Preferences

/// User preferences, stored in UserDefaults under stable keys so the Settings
/// window (`@AppStorage`) and the model code (`Prefs.*`) read the same values.
enum Prefs {
    static let soundsKey = "vcad.prefs.sounds"
    static let hapticsKey = "vcad.prefs.haptics"
    static let gridKey = "vcad.prefs.grid"
    static let triadKey = "vcad.prefs.triad"
    static let presentationKey = "vcad.prefs.presentation"

    private static var defaults: UserDefaults { .standard }
    private static func bool(_ key: String, default value: Bool) -> Bool {
        defaults.object(forKey: key) == nil ? value : defaults.bool(forKey: key)
    }

    /// Play the solve / verdict chimes.
    static var playsSounds: Bool { bool(soundsKey, default: true) }
    /// Haptic detents while scrubbing values.
    static var haptics: Bool { bool(hapticsKey, default: true) }
    /// Draw the reference grid and contact shadow in the windowed viewport.
    static var showsGrid: Bool { bool(gridKey, default: true) }
    /// Draw the orientation triad in the viewport corner.
    static var showsTriad: Bool { bool(triadKey, default: true) }
    /// Open new instances in a window rather than released over the desktop.
    static var opensInWindow: Bool { bool(presentationKey, default: false) }
}

/// What the app remembers about its layout between launches: the active
/// workspace, which panels are open, and the window presentation.
struct LayoutMemory: Codable, Equatable {
    static let key = "vcad.layout"
    var workspace = "design"
    var showsTree = true
    var showsInspector = true
    var cncLeft = true
    var cncRight = true
    var cncBottom = false
    var measurementsShown = false
    var windowed = false

    static func load() -> LayoutMemory {
        guard let data = UserDefaults.standard.data(forKey: key),
              let m = try? JSONDecoder().decode(LayoutMemory.self, from: data) else { return LayoutMemory() }
        return m
    }
    func save() {
        if let data = try? JSONEncoder().encode(self) {
            UserDefaults.standard.set(data, forKey: Self.key)
        }
    }
}
