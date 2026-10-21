import SwiftUI

// The Agent panel: a chat over the open document. The model (frog api or a
// local OpenAI-compatible server, see FrogClient.swift) edits through the
// engine's tool lowering; each edit shows in the viewport as it lands, and a
// whole turn is one undo step ("Agent Edit").

@MainActor
@Observable
final class AgentSession {
    static let shared = AgentSession()

    struct ToolChip: Identifiable, Equatable {
        let id: String
        let name: String
        var state: State
        var detail: String
        enum State: Equatable { case running, ok, failed }
    }

    struct Entry: Identifiable, Equatable {
        enum Role: Equatable { case user, assistant, error }
        let id = UUID()
        let role: Role
        var text: String
        var tools: [ToolChip] = []
    }

    var isOpen = false
    var draft = ""
    private(set) var entries: [Entry] = []
    private(set) var running = false

    @ObservationIgnored private var agent: FrogAgent?
    @ObservationIgnored private weak var agentEngine: DocumentEngine?
    @ObservationIgnored private var task: Task<Void, Never>?

    func toggle() { isOpen.toggle() }

    func clear() {
        stop()
        entries = []
        agent = nil
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func submit(model: EditorModel) {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty, !running else { return }
        guard let engine = model.agentEnsureEngine() else {
            entries.append(Entry(role: .error, text: "This document is view-only; the agent can't edit it."))
            return
        }
        draft = ""
        entries.append(Entry(role: .user, text: text))
        entries.append(Entry(role: .assistant, text: ""))
        if agent == nil || agentEngine !== engine {
            let config = FrogConfig.load()
            let executor = FrogToolExecutor(engine: engine, supportsImages: config.supportsImages,
                                            snapshot: { [weak model] view in
                                                guard let model else { return nil }
                                                return await Self.snapshot(view, model: model)
                                            })
            agent = FrogAgent(client: FrogClient(config: config), manifest: .load(), executor: executor)
            agentEngine = engine
        }
        guard let agent else { return }
        running = true
        let before = engine.save()
        task = Task { [weak self] in
            var mutated = false
            do {
                try await agent.send(text) { event in
                    if case .toolFinished(_, let o) = event, o.mutation != nil { mutated = true }
                    self?.handle(event, model: model)
                }
            } catch is CancellationError {
                self?.appendToLast("(stopped)")
            } catch {
                let message = (error as? LocalizedError)?.errorDescription ?? String(describing: error)
                if let last = self?.entries.last, last.role == .assistant, last.text.isEmpty, last.tools.isEmpty {
                    self?.entries.removeLast()
                }
                self?.entries.append(Entry(role: .error, text: message))
            }
            if mutated { model.agentEndTurn(before: before) }
            self?.running = false
            self?.task = nil
        }
    }

    private func handle(_ event: FrogTurnEvent, model: EditorModel) {
        guard let last = entries.indices.last else { return }
        switch event {
        case .text(let t):
            entries[last].text += t
        case let .toolStarted(id, name, args):
            entries[last].tools.append(ToolChip(id: id, name: name, state: .running, detail: args))
        case let .toolFinished(id, outcome):
            if let r = outcome.mutation { model.agentApplied(r) }
            if let i = entries[last].tools.firstIndex(where: { $0.id == id }) {
                entries[last].tools[i].state = outcome.ok ? .ok : .failed
                entries[last].tools[i].detail = outcome.content
            }
        }
    }

    /// Render `view` without leaving the user's camera moved: swing to the
    /// requested angle, capture once the viewport has redrawn, swing back.
    static func snapshot(_ view: FrogView, model: EditorModel) async -> Data? {
        guard case let .angles(az, el) = view else { return await AgentTools.snapshotPNG() }
        model.stopSpin()
        let saved = (model.azimuth, model.elevation)
        model.azimuth = Float(az * .pi / 180)
        model.elevation = Float(el * .pi / 180)
        try? await Task.sleep(for: .milliseconds(150))
        let png = await AgentTools.snapshotPNG()
        model.azimuth = saved.0
        model.elevation = saved.1
        return png
    }

    private func appendToLast(_ s: String) {
        guard let last = entries.indices.last else { return }
        entries[last].text += entries[last].text.isEmpty ? s : "\n\(s)"
    }
}

struct AgentPanel: View {
    @Bindable var session: AgentSession
    let model: EditorModel
    @FocusState private var focused: Bool

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            PanelHeader(title: "Agent", systemImage: "sparkles") { session.isOpen = false }
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: Theme.Space.s) {
                        if session.entries.isEmpty {
                            EmptyPanelState(title: "Ask for a change", systemImage: "text.bubble",
                                            detail: "e.g. “add a 40 mm aluminum plate under the bracket”")
                        }
                        ForEach(session.entries) { entry in
                            AgentEntryView(entry: entry).id(entry.id)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .frame(maxHeight: 320)
                .onChange(of: session.entries.last?.text) {
                    if let id = session.entries.last?.id { proxy.scrollTo(id, anchor: .bottom) }
                }
            }
            HStack(spacing: Theme.Space.s) {
                TextField("Ask the agent to edit this document", text: $session.draft, axis: .vertical)
                    .textFieldStyle(.plain)
                    .lineLimit(1...4)
                    .focused($focused)
                    .onSubmit { session.submit(model: model) }
                    .padding(Theme.Space.s)
                    .controlWell(active: focused)
                    .disabled(!model.agentCanEdit)
                if session.running {
                    Button { session.stop() } label: { Image(systemName: "stop.circle.fill").font(.title2) }
                        .buttonStyle(.borderless)
                        .help("Stop the agent")
                        .accessibilityLabel("Stop")
                } else {
                    Button { session.submit(model: model) } label: { Image(systemName: "arrow.up.circle.fill").font(.title2) }
                        .buttonStyle(.borderless)
                        .disabled(session.draft.trimmingCharacters(in: .whitespaces).isEmpty || !model.agentCanEdit)
                        .help("Send")
                        .accessibilityLabel("Send")
                }
            }
            if !model.agentCanEdit {
                Text("This document is view-only; the agent can't edit it.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
        .padding(Theme.Space.m)
        .frame(width: 420)
        .panelSurface()
        .onAppear { focused = true }
    }
}

private struct AgentEntryView: View {
    let entry: AgentSession.Entry

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            if !entry.tools.isEmpty {
                FlowChips(tools: entry.tools)
            }
            if !entry.text.isEmpty || entry.role != .assistant {
                Text(entry.text)
                    .font(.callout)
                    .foregroundStyle(entry.role == .error ? AnyShapeStyle(.red) : entry.role == .user ? AnyShapeStyle(.secondary) : AnyShapeStyle(.primary))
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: entry.role == .user ? .trailing : .leading)
            }
        }
    }
}

private struct FlowChips: View {
    let tools: [AgentSession.ToolChip]
    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(tools) { chip in
                HStack(spacing: Theme.Space.xs) {
                    switch chip.state {
                    case .running: ProgressView().controlSize(.mini)
                    case .ok: Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                    case .failed: Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
                    }
                    Text(chip.name).font(.caption.monospaced())
                }
                .padding(.horizontal, Theme.Space.s).padding(.vertical, 2)
                .background(.quaternary, in: Capsule())
                .help(chip.detail)
            }
        }
    }
}

#if os(macOS)
/// Settings ▸ Agent: endpoint, model id, key (Keychain), image capability.
struct AgentSettingsForm: View {
    @AppStorage(FrogConfig.baseKey) private var baseURL = ""
    @AppStorage(FrogConfig.modelKey) private var modelID = ""
    @AppStorage(FrogConfig.imagesKey) private var supportsImages = false
    @State private var key = ""
    @State private var keySaved = FrogKeychain.shared.read() != nil

    var body: some View {
        Section {
            TextField("Endpoint", text: $baseURL, prompt: Text(FrogConfig.defaultBaseURL))
            TextField("Model", text: $modelID, prompt: Text("From the endpoint"))
            HStack {
                SecureField("API key", text: $key, prompt: Text(keySaved ? "Saved in Keychain" : "frog_…"))
                Button("Save") { keySaved = FrogKeychain.shared.write(key) && !key.isEmpty; key = "" }
                    .disabled(key.isEmpty)
                Button("Remove") { FrogKeychain.shared.write(nil); keySaved = false }
                    .disabled(!keySaved)
            }
            if let warning = FrogConfig.keyWarning(key) {
                Label(warning, systemImage: "exclamationmark.triangle").font(.caption).foregroundStyle(.orange)
            }
            Toggle("Endpoint accepts images (send viewport snapshots)", isOn: $supportsImages)
        } header: {
            Text("frog api")
        } footer: {
            Text("Any OpenAI-compatible endpoint works, including a local tang-llm server. The key is stored in your Keychain. Leave Model empty to use the one the endpoint lists. Environment overrides: FROG_BASE_URL, FROG_API_KEY, FROG_MODEL, FROG_SUPPORTS_IMAGES.")
                .font(.caption).foregroundStyle(.secondary)
        }
    }
}
#endif
