#if canImport(AppKit)
import AppKit
#endif
import CVcadFFI
import CoreServices
import Foundation
import SwiftUI

// The loon-first loop: open a `.loon` file (or a folder holding one), edit it
// in any text editor, and every save rebuilds the model in place.
//
//  - errors land inline as `file:line:column` with a jump to the editor;
//  - `[use]` modules resolve against the file's directory, then
//    `$VCAD_LOON_PATH` (vcad-loon's module resolver), and a save to any loon
//    file under those directories rebuilds too;
//  - an edit that fails to compile leaves the last good model on screen.

/// A loon compile error with its source location, when one is attributable.
struct LoonDiagnostic: Error, Equatable, Sendable, Decodable {
    var message: String
    var file: String?
    var line: Int?
    var column: Int?

    /// `main.loon:12:5`, or just the file name, or nil.
    var location: String? {
        guard let file else { return nil }
        let name = (file as NSString).lastPathComponent
        switch (line, column) {
        case let (l?, c?): return "\(name):\(l):\(c)"
        case let (l?, nil): return "\(name):\(l)"
        default: return name
        }
    }
}

enum LoonCompiler {
    /// Compile a loon file to `.vcad` JSON, or say where it failed.
    nonisolated static func compile(path: String) -> Result<Data, LoonDiagnostic> {
        let bytes = Array(path.utf8)
        var diag: UnsafeMutablePointer<CChar>?
        let json = bytes.withUnsafeBufferPointer {
            vcad_loon_file_to_json_diag($0.baseAddress, $0.count, &diag)
        }
        if let json {
            defer { vcad_cam_free(json) }
            return .success(Data(bytes: json, count: strlen(json)))
        }
        guard let diag else {
            return .failure(LoonDiagnostic(message: vcadLastError("did not compile"), file: path))
        }
        defer { vcad_cam_free(diag) }
        let data = Data(bytes: diag, count: strlen(diag))
        let decoded = (try? JSONDecoder().decode(LoonDiagnostic.self, from: data))
            ?? LoonDiagnostic(message: String(decoding: data, as: UTF8.self), file: path)
        return .failure(decoded)
    }

    /// The program a folder stands for: `main.loon`, else `<folder>.loon`,
    /// else its only top-level `.loon` file.
    static func entryFile(inFolder dir: URL) -> URL? {
        let fm = FileManager.default
        for name in ["main.loon", "\(dir.lastPathComponent).loon"] {
            let u = dir.appendingPathComponent(name)
            if fm.fileExists(atPath: u.path) { return u }
        }
        let loons = (try? fm.contentsOfDirectory(at: dir, includingPropertiesForKeys: nil))?
            .filter { $0.pathExtension.lowercased() == "loon" } ?? []
        return loons.count == 1 ? loons[0] : nil
    }

    /// Directories whose loon files feed a program at `path`: its own
    /// directory, then each `$VCAD_LOON_PATH` entry that exists.
    static func watchedDirectories(for path: String,
                                   environment: [String: String] = ProcessInfo.processInfo.environment) -> [String] {
        var dirs = [(path as NSString).deletingLastPathComponent]
        if let lib = environment["VCAD_LOON_PATH"] {
            for d in lib.split(separator: ":").map(String.init) where !d.isEmpty {
                var isDir: ObjCBool = false
                if FileManager.default.fileExists(atPath: d, isDirectory: &isDir), isDir.boolValue,
                   !dirs.contains(d) {
                    dirs.append(d)
                }
            }
        }
        return dirs
    }
}

/// FSEvents watch over a set of directories, reporting changes to loon (and
/// mesh) files. Unlike a vnode watch on the one file, it sees a module edited
/// in place and survives editors that save by replace-and-rename.
final class LoonWatcher {
    private var stream: FSEventStreamRef?
    private let onChange: () -> Void

    init?(directories: [String], onChange: @escaping () -> Void) {
        self.onChange = onChange
        var ctx = FSEventStreamContext(version: 0, info: nil, retain: nil, release: nil, copyDescription: nil)
        ctx.info = Unmanaged.passUnretained(self).toOpaque()
        let callback: FSEventStreamCallback = { _, info, count, paths, _, _ in
            guard let info else { return }
            let watcher = Unmanaged<LoonWatcher>.fromOpaque(info).takeUnretainedValue()
            let list = unsafeBitCast(paths, to: NSArray.self) as? [String] ?? []
            let relevant = list.prefix(count).contains { p in
                let ext = (p as NSString).pathExtension.lowercased()
                return ["loon", "stl", "step", "stp", "obj", "glb"].contains(ext)
            }
            if relevant { watcher.onChange() }
        }
        guard let s = FSEventStreamCreate(nil, callback, &ctx, directories as CFArray,
                                          FSEventStreamEventId(kFSEventStreamEventIdSinceNow), 0.15,
                                          FSEventStreamCreateFlags(kFSEventStreamCreateFlagFileEvents
                                                                   | kFSEventStreamCreateFlagUseCFTypes
                                                                   | kFSEventStreamCreateFlagNoDefer))
        else { return nil }
        stream = s
        FSEventStreamSetDispatchQueue(s, .main)
        FSEventStreamStart(s)
    }

    func cancel() {
        guard let s = stream else { return }
        FSEventStreamStop(s)
        FSEventStreamInvalidate(s)
        FSEventStreamRelease(s)
        stream = nil
    }

    deinit { cancel() }
}

#if os(macOS)
/// Open `file` at `line:column` in the user's editor: `$VCAD_EDITOR_URL`
/// (a template with `{file}`, `{line}`, `{column}`), else VS Code, Cursor or
/// Zed when installed, else the file's default app.
@MainActor func revealLoonDiagnostic(_ d: LoonDiagnostic) {
    guard let file = d.file else { return }
    let line = d.line ?? 1, column = d.column ?? 1
    let encoded = file.addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? file
    var templates: [String] = []
    if let custom = ProcessInfo.processInfo.environment["VCAD_EDITOR_URL"] { templates.append(custom) }
    templates += ["vscode://file{file}:{line}:{column}", "cursor://file{file}:{line}:{column}",
                  "zed://file{file}:{line}:{column}"]
    for t in templates {
        let s = t.replacingOccurrences(of: "{file}", with: encoded)
            .replacingOccurrences(of: "{line}", with: "\(line)")
            .replacingOccurrences(of: "{column}", with: "\(column)")
        if let url = URL(string: s), NSWorkspace.shared.urlForApplication(toOpen: url) != nil {
            NSWorkspace.shared.open(url)
            return
        }
    }
    NSWorkspace.shared.open(URL(fileURLWithPath: file))
}
#endif

extension EditorModel {
    /// Start (or move) the loon watch for the open document. No-op for any
    /// other document, which keeps the single-file vnode watch.
    func watchLoonSources() {
        guard case let .document(path, _) = source, path.lowercased().hasSuffix(".loon") else {
            loonWatcher?.cancel(); loonWatcher = nil; loonWatchPath = nil
            return
        }
        guard loonWatchPath != path else { return }
        loonWatcher?.cancel()
        loonWatcher = LoonWatcher(directories: LoonCompiler.watchedDirectories(for: path)) { [weak self] in
            MainActor.assumeIsolated { self?.scheduleLoonRebuild() }
        }
        loonWatchPath = path
    }

    private func scheduleLoonRebuild() {
        reloadPending?.cancel()
        let work = DispatchWorkItem { [weak self] in
            MainActor.assumeIsolated { self?.rebuildLoon() }
        }
        reloadPending = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
    }

    /// Recompile the open loon program. A failure keeps the last good model
    /// on screen and shows where the program broke; a success replaces it.
    func rebuildLoon() {
        guard case let .document(path, _) = source, path.lowercased().hasSuffix(".loon"),
              FileManager.default.fileExists(atPath: path) else { return }
        switch LoonCompiler.compile(path: path) {
        case .failure(let d):
            loonDiagnostic = d
        case .success(let data):
            loonDiagnostic = nil
            precompiledLoon = (path, data)
            revertDocument()
            lastReload = Date()
        }
    }
}

/// The inline compile-error pill over the viewport.
struct LoonDiagnosticBanner: View {
    let diagnostic: LoonDiagnostic
    let keptLastGood: Bool

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: Theme.Space.s) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange)
            VStack(alignment: .leading, spacing: 2) {
                if let loc = diagnostic.location {
                    Text(loc).font(.callout.monospaced().weight(.semibold))
                }
                Text(diagnostic.message).font(.callout).lineLimit(4).textSelection(.enabled)
                if keptLastGood {
                    Text("Showing the last version that compiled.").font(.caption).foregroundStyle(.secondary)
                }
            }
            #if os(macOS)
            if diagnostic.file != nil {
                Button(diagnostic.line != nil ? "Go to Line" : "Open File") { revealLoonDiagnostic(diagnostic) }
                    .controlSize(.small)
            }
            #endif
        }
        .padding(.horizontal, 14).padding(.vertical, 8).pillSurface()
        .frame(maxWidth: 560)
        .accessibilityElement(children: .combine)
    }
}
