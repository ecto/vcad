#if os(macOS)
import AppKit
#endif
import Foundation
import SwiftUI

// First run: a welcome window with sample projects bundled in the app and a
// short "your first part in loon" tutorial. Samples live in
// apple/VcadApp/Samples (bundle.sh copies them to Contents/Resources/samples)
// and are described by samples.json there.

/// One bundled sample.
struct SampleProject: Decodable, Identifiable, Equatable, Sendable {
    let id: String
    let title: String
    let subtitle: String
    let file: String
    let thumbnail: String
    var tutorial: Bool? = nil
}

enum SampleLibrary {
    /// Where the samples are: `$VCAD_SAMPLES`, the app bundle's
    /// `Resources/samples`, or `apple/VcadApp/Samples` in a checkout above the
    /// running binary (a dev build).
    static func directory(environment: [String: String] = ProcessInfo.processInfo.environment,
                          bundle: Bundle = .main) -> URL? {
        let fm = FileManager.default
        var candidates: [URL] = []
        if let env = environment["VCAD_SAMPLES"], !env.isEmpty { candidates.append(URL(fileURLWithPath: env)) }
        if let res = bundle.resourceURL { candidates.append(res.appendingPathComponent("samples")) }
        var dir = bundle.bundleURL
        for _ in 0..<8 {
            dir = dir.deletingLastPathComponent()
            candidates.append(dir.appendingPathComponent("apple/VcadApp/Samples"))
            candidates.append(dir.appendingPathComponent("Samples"))
        }
        return candidates.first { fm.fileExists(atPath: $0.appendingPathComponent("samples.json").path) }
    }

    /// The samples listed in `dir/samples.json` whose files exist.
    static func load(from dir: URL) -> [SampleProject] {
        guard let data = try? Data(contentsOf: dir.appendingPathComponent("samples.json")),
              let all = try? JSONDecoder().decode([SampleProject].self, from: data) else { return [] }
        let fm = FileManager.default
        return all.filter { fm.fileExists(atPath: dir.appendingPathComponent($0.file).path) }
    }

    /// Where opened samples are copied, so they can be edited (the app
    /// bundle is read-only and signed): `~/Documents/vcad/Samples`.
    static var workspace: URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Documents/vcad/Samples", isDirectory: true)
    }

    /// Copy a sample into `workspace` (once; an edited copy is never
    /// overwritten) and return the copy.
    static func materialize(_ sample: SampleProject, from dir: URL, into workspace: URL = workspace) throws -> URL {
        let fm = FileManager.default
        try fm.createDirectory(at: workspace, withIntermediateDirectories: true)
        let dest = workspace.appendingPathComponent(sample.file)
        if !fm.fileExists(atPath: dest.path) {
            // Read through any symlink (the checkout links repo examples).
            let data = try Data(contentsOf: dir.appendingPathComponent(sample.file).resolvingSymlinksInPath())
            try data.write(to: dest, options: .atomic)
        }
        return dest
    }
}

#if os(macOS)
/// The welcome window: shown once on first launch, and from Help ▸ Welcome to vcad.
@MainActor
final class WelcomeWindowController {
    static let shared = WelcomeWindowController()
    static let shownKey = "vcad.welcome.shown"
    private var window: NSWindow?

    /// First launch of a scratch instance, outside the headless dev hooks.
    static func shouldShowAtLaunch(model: EditorModel,
                                   defaults: UserDefaults = .standard,
                                   environment: [String: String] = ProcessInfo.processInfo.environment) -> Bool {
        guard !defaults.bool(forKey: shownKey), model.isDisposableScratch else { return false }
        if environment["VCAD_NO_WELCOME"] == "1" { return false }
        // Smoke/dev hooks drive the app without a person in front of it.
        return !environment.keys.contains { $0.hasPrefix("VCAD_") && $0 != "VCAD_LOON_PATH" && $0 != "VCAD_SAMPLES" }
    }

    func show(model: EditorModel) {
        UserDefaults.standard.set(true, forKey: Self.shownKey)
        if let window { window.makeKeyAndOrderFront(nil); return }
        let dir = SampleLibrary.directory()
        let samples = dir.map(SampleLibrary.load(from:)) ?? []
        let view = WelcomeView(samples: samples, directory: dir) { [weak self] sample in
            guard let dir else { return }
            do {
                let url = try SampleLibrary.materialize(sample, from: dir)
                AppInstance.opening(url, from: model)
                if sample.tutorial == true { NSWorkspace.shared.open(url) }   // the editor, beside the model
                self?.window?.close()
            } catch {
                model.loadError = "Could not open \(sample.title): \(error.localizedDescription)"
            }
        } onOpen: { [weak self] in
            self?.window?.close()
            openPanel(model)
        }
        let w = NSWindow(contentViewController: NSHostingController(rootView: view))
        w.title = "Welcome to vcad"
        w.styleMask = [.titled, .closable, .fullSizeContentView]
        w.titlebarAppearsTransparent = true
        w.isReleasedWhenClosed = false
        w.center()
        window = w
        w.makeKeyAndOrderFront(nil)
    }
}

struct WelcomeView: View {
    let samples: [SampleProject]
    let directory: URL?
    let onSample: (SampleProject) -> Void
    let onOpen: () -> Void

    private var tutorial: SampleProject? { samples.first { $0.tutorial == true } }
    private var projects: [SampleProject] { samples.filter { $0.tutorial != true } }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.l) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Welcome to vcad").font(.largeTitle.weight(.semibold))
                Text("Parametric CAD you can write. Start with a sample, or open your own .vcad or .loon.")
                    .foregroundStyle(.secondary)
            }
            if let tutorial {
                Button { onSample(tutorial) } label: {
                    HStack(spacing: Theme.Space.m) {
                        thumbnail(tutorial, height: 120).frame(width: 160)
                        VStack(alignment: .leading, spacing: 4) {
                            Text(tutorial.title).font(.title3.weight(.semibold))
                            Text(tutorial.subtitle).foregroundStyle(.secondary)
                            Text("Opens the model and the source side by side.").font(.caption).foregroundStyle(.tertiary)
                        }
                        Spacer()
                    }
                    .padding(Theme.Space.m)
                    .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 10))
                }
                .buttonStyle(.plain)
            }
            Text("Samples").font(.headline)
            LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: Theme.Space.m, alignment: .top), count: 4),
                      alignment: .leading, spacing: Theme.Space.m) {
                ForEach(projects) { s in
                    Button { onSample(s) } label: {
                        VStack(alignment: .leading, spacing: 6) {
                            thumbnail(s, height: 110)
                            Text(s.title).font(.callout.weight(.semibold))
                            Text(s.subtitle).font(.caption).foregroundStyle(.secondary).lineLimit(3)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel("Open sample \(s.title)")
                }
            }
            if samples.isEmpty {
                Text("No samples found in this build.").foregroundStyle(.secondary)
            }
            HStack {
                Button("Open…") { onOpen() }
                Spacer()
                Text("Samples are copied to ~/Documents/vcad/Samples so you can edit them.")
                    .font(.caption).foregroundStyle(.tertiary)
            }
        }
        .padding(28)
        .frame(width: 760)
    }

    @ViewBuilder private func thumbnail(_ s: SampleProject, height: CGFloat) -> some View {
        if let dir = directory, let image = NSImage(contentsOf: dir.appendingPathComponent(s.thumbnail)) {
            Image(nsImage: image).resizable().aspectRatio(contentMode: .fill)
                .frame(minWidth: 0, maxWidth: .infinity, minHeight: height, maxHeight: height)
                .clipShape(RoundedRectangle(cornerRadius: 8))
        } else {
            RoundedRectangle(cornerRadius: 8).fill(.quaternary).frame(height: height)
        }
    }
}
#endif
