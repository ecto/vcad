import SwiftUI

// The AI intent bar — the flagship. Type a description in the title-bar command
// field; an agent (Claude) emits a loon program; the kernel compiles + evaluates
// it through vcad_scene_from_loon; the studio renders it with the materialize
// pop. This is the visible embodiment of "add the brain": the parametric kernel
// reached by intent, not just by hand.

@MainActor
@Observable
final class IntentEngine {
    enum Phase: Equatable {
        case idle
        case thinking
        case done(String)     // success summary, e.g. "Built 3 parts"
        case failed(String)   // human-readable error
    }

    var phase: Phase = .idle
    var draft: String = ""
    /// Set by the example chips to pull focus into the field after filling it.
    var focusRequested = false

    private let client = VcadIntentClient()
    private var inFlight: Task<Void, Never>?

    var isThinking: Bool { phase == .thinking }

    /// Kick off a generation for the current draft. No-ops on empty input. The
    /// backend holds the credentials, so there's nothing to check here.
    func submit(into model: EditorModel) {
        let prompt = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !prompt.isEmpty else { return }
        inFlight?.cancel()
        phase = .thinking
        inFlight = Task { await run(prompt: prompt, into: model) }
    }

    private func run(prompt: String, into model: EditorModel) async {
        do {
            let reply = try await client.complete(system: Self.systemPrompt, user: prompt)
            try Task.checkCancellation()
            let loon = Self.extractLoon(reply)
            guard !loon.isEmpty else {
                phase = .failed("The model returned no geometry.")
                return
            }
            // Validate + adopt. A program that doesn't evaluate leaves the
            // current studio untouched rather than blanking it.
            if let stats = model.applyGenerated(loon: loon, label: Self.label(from: prompt)) {
                draft = ""
                phase = .done(Self.summary(stats))
                try? await Task.sleep(for: .seconds(2.6))
                if case .done = phase { phase = .idle }
            } else {
                phase = .failed("That didn't evaluate to valid geometry — try rephrasing.")
            }
        } catch is CancellationError {
            phase = .idle
        } catch {
            let msg = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            phase = .failed(msg)
        }
    }

    func cancel() {
        inFlight?.cancel()
        inFlight = nil
        phase = .idle
    }

    func dismissError() {
        if case .failed = phase { phase = .idle }
    }

    // MARK: reply parsing

    /// Pull the loon program out of a model reply — unwrapping a fenced code
    /// block if present, else taking the trimmed text.
    static func extractLoon(_ reply: String) -> String {
        if let fenced = firstFencedBlock(in: reply) { return fenced }
        return reply.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func firstFencedBlock(in s: String) -> String? {
        guard let open = s.range(of: "```") else { return nil }
        let afterOpen = s[open.upperBound...]
        guard let close = afterOpen.range(of: "```") else { return nil }
        var body = String(afterOpen[..<close.lowerBound])
        // Drop a leading language-tag line ("loon\n") if the fence carried one.
        if let nl = body.firstIndex(of: "\n") {
            let first = body[body.startIndex..<nl].trimmingCharacters(in: .whitespaces)
            if first.isEmpty || (!first.contains("[") && first.count < 12) {
                body = String(body[body.index(after: nl)...])
            }
        }
        return body.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// A short feature-tree label derived from the prompt.
    static func label(from prompt: String) -> String {
        let cleaned = prompt.replacingOccurrences(of: "\n", with: " ")
            .trimmingCharacters(in: .whitespaces)
        if cleaned.isEmpty { return "Generated" }
        if cleaned.count <= 30 { return cleaned }
        return String(cleaned.prefix(29)).trimmingCharacters(in: .whitespaces) + "…"
    }

    /// One-line confirmation for a finished build.
    static func summary(_ s: GenStats) -> String {
        let d = s.size
        let dims = String(format: "%.0f×%.0f×%.0f mm", abs(d.x), abs(d.y), abs(d.z))
        return "Built · \(s.parts) part\(s.parts == 1 ? "" : "s") · \(dims)"
    }

    /// Seed suggestions shown as chips over an untouched studio.
    static let examplePrompts = [
        "a 40mm motor mount",
        "a phone stand",
        "a hex standoff, 12mm across flats",
        "a rounded enclosure lid",
        "an L-bracket with bolt holes",
    ]

    // MARK: system prompt

    /// Teaches the model to author loon — the same vocabulary the vcad MCP's
    /// `create_cad_loon` tool uses, with worked examples. The reply is fed
    /// straight to the kernel, so the contract is "emit a program, nothing else."
    static let systemPrompt = """
    You turn a natural-language description of a mechanical part into a `loon` program for vcad, a parametric CAD kernel. Your program is compiled and evaluated directly — there is no human to fix it.

    OUTPUT CONTRACT
    - Respond with ONLY a loon program. No prose, no explanation.
    - A fenced ```loon code block is fine; put nothing outside it.
    - The program MUST end with at least one [root <solid> "<material>"] — that is what becomes visible geometry.

    CONVENTIONS
    - Units are millimeters. Coordinate system is Z-up: X right, Y forward, Z up.
    - [cube x y z] has its corner at the origin and extends to (x, y, z); z is height.
    - [cylinder r h] and [cone rb rt h] have their axis along Z (already upright).
    - Keep parts roughly 5–200 mm so they frame well.

    VOCABULARY (loon is Lisp-like; most ops take the subject LAST so they thread through [pipe …])
    Primitives: [cube x y z]  [cylinder r h]  [sphere r]  [cone r-bottom r-top h]
    Booleans (subject-last): [difference tool subject]  [union other subject]  [intersection other subject]
    Transforms (subject-last): [translate x y z s]  [rotate rx ry rz s]  [scale sx sy sz s]   (angles in degrees)
    Features (subject-last): [fillet r s]  [chamfer d s]  [shell t s]
    Patterns (subject-last):
      [linear-pattern dx dy dz count spacing s]
      [circular-pattern ox oy oz ax ay az count angle s]   (a bolt circle: [circular-pattern 0 0 0 0 0 1 6 360 hole])
    Sketches: [sketch ox oy oz xx xy xz yx yy yz #[segments]] with [line x1 y1 x2 y2] and [arc x1 y1 x2 y2 cx cy ccw]
    Sketch ops (sketch-last): [extrude dx dy dz sk]  [revolve aox aoy aoz adx ady adz angle sk]  [sweep-line sx sy sz ex ey ez sk]  [sweep-helix radius pitch height turns sk]  [loft #[sk1 sk2 …]]
    Bindings: [let name value]      Pipe: [pipe [cube 50 30 5] [difference [cylinder 3 10]] [fillet 1.0]]
    Scene: [root solid "material"] — one root per visible part.

    MATERIALS (choose one that fits): aluminum steel brass copper titanium chrome gold silver  abs-white abs-black abs-red abs-blue pla petg nylon resin acrylic rubber  oak walnut  glass glass-tinted  carbon-fiber  concrete ceramic foam

    EXAMPLES

    Mounting plate with corner holes:
    [let plate [cube 100 60 5]]
    [let hole [cylinder 3 10]]
    [let holes
      [pipe hole
        [translate 5 5 -2.5]
        [union [translate 95 5 -2.5 hole]]
        [union [translate 5 55 -2.5 hole]]
        [union [translate 95 55 -2.5 hole]]]]
    [root [difference holes plate] "aluminum"]

    L-bracket from a sketch, then fillet:
    [let sk [sketch
      0 0 0  1 0 0  0 1 0
      #[[line 0 0 10 0] [line 10 0 10 5] [line 10 5 0 5] [line 0 5 0 0]]]]
    [root [fillet 1.5 [extrude 0 0 20 sk]] "steel"]

    Filleted cylinder:
    [root [fillet 2.0 [cylinder 10 30]] "brass"]

    Build exactly what the user asks for. Prefer simple, clean, correct geometry over cleverness. Use [let …] to name reused solids. If a dimension is unspecified, choose a sensible default.
    """
}

// MARK: - Command bar (title-bar center)

/// The always-present command field. ⌘K focuses it; Return submits. State
/// (idle / thinking / done / failed) renders inline within a fixed-width pill so
/// it never jostles the rest of the toolbar.
struct CommandBar: View {
    @Bindable var engine: IntentEngine
    let model: EditorModel
    @FocusState private var focused: Bool

    var body: some View {
        HStack(spacing: 8) { stateContent }
            .frame(width: 380, height: 22)
            .padding(.horizontal, 11)
            .padding(.vertical, 5)
            .background(.quaternary.opacity(0.55), in: Capsule(style: .continuous))
            .overlay(Capsule(style: .continuous).strokeBorder(borderColor, lineWidth: 1))
            .animation(.smooth(duration: 0.24), value: engine.phase)
            .animation(.snappy(duration: 0.2), value: focused)
            .background(shortcutButton)
            .onExitCommand { engine.dismissError(); focused = false }
            .onChange(of: engine.focusRequested) { _, req in
                if req { focused = true; engine.focusRequested = false }
            }
    }

    @ViewBuilder private var stateContent: some View {
        switch engine.phase {
        case .idle:
            Image(systemName: "sparkles").font(.system(size: 12)).foregroundStyle(.tint)
            TextField("Describe a part…", text: $engine.draft)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .focused($focused)
                .onSubmit { engine.submit(into: model) }
            if engine.draft.isEmpty {
                KeycapView("⌘K")
            } else {
                Button { engine.submit(into: model) } label: {
                    Image(systemName: "arrow.up.circle.fill").font(.system(size: 16))
                }
                .buttonStyle(.plain).foregroundStyle(.tint)
                .help("Build (Return)")
            }

        case .thinking:
            Image(systemName: "sparkles").font(.system(size: 12)).foregroundStyle(.tint)
                .symbolEffect(.pulse, options: .repeating)
            Text("Designing…").font(.system(size: 13))
            Spacer(minLength: 0)
            ProgressView().controlSize(.small).scaleEffect(0.78)
            Button { engine.cancel() } label: {
                Image(systemName: "xmark.circle.fill").font(.system(size: 14))
            }
            .buttonStyle(.plain).foregroundStyle(.secondary).help("Cancel")

        case .done(let summary):
            Image(systemName: "checkmark.circle.fill").font(.system(size: 13)).foregroundStyle(.green)
            Text(summary).font(.system(size: 13))
            Spacer(minLength: 0)

        case .failed(let message):
            Image(systemName: "exclamationmark.triangle.fill").font(.system(size: 12)).foregroundStyle(.orange)
            Text(message).font(.system(size: 12)).foregroundStyle(.secondary)
                .lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 0)
            Button { engine.dismissError(); focused = true } label: {
                Image(systemName: "arrow.counterclockwise").font(.system(size: 12))
            }
            .buttonStyle(.plain).foregroundStyle(.secondary).help("Try again")
        }
    }

    private var borderColor: Color {
        if engine.isThinking { return .accentColor.opacity(0.6) }
        if focused { return .accentColor.opacity(0.45) }
        if case .failed = engine.phase { return .orange.opacity(0.4) }
        return .white.opacity(0.10)
    }

    /// Invisible ⌘K accelerator that focuses the field from anywhere.
    private var shortcutButton: some View {
        Button("") {
            if case .failed = engine.phase { engine.dismissError() }
            focused = true
        }
        .keyboardShortcut("k", modifiers: .command)
        .opacity(0).frame(width: 0, height: 0).accessibilityHidden(true)
    }
}

/// A small rounded keycap hint (e.g. ⌘K).
struct KeycapView: View {
    let text: String
    init(_ text: String) { self.text = text }
    var body: some View {
        Text(text)
            .font(.system(size: 10, weight: .medium, design: .rounded))
            .foregroundStyle(.secondary)
            .padding(.horizontal, 5).padding(.vertical, 1.5)
            .background(.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 4, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 4, style: .continuous)
                .strokeBorder(.white.opacity(0.12), lineWidth: 0.5))
    }
}
