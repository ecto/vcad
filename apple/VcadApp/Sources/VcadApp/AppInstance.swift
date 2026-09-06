#if canImport(AppKit)
import AppKit
#endif
import Foundation

/// Opening a second document opens a second COPY OF THE APP.
///
/// The ask was "each tab is another icon in the Dock, and ⌘-Tab shows them
/// separately". macOS does not offer that within one process: the Dock tile and
/// the ⌘-Tab entry belong to the *application*, not to its windows, so a
/// document-per-window app (Pages, Xcode) shows exactly one of each no matter
/// how many documents are open. `NSApp.dockTile` customises that single tile; it
/// does not mint new ones. There is no supported API for a second tile short of
/// a second app bundle — which would mean shipping a decoy bundle pretending to
/// be vcad, and would still be one tile per bundle rather than per document.
///
/// So a document is an app INSTANCE. That is a real fit rather than a
/// workaround here: release-to-desktop has no window of its own — the app *is*
/// the parts floating over the desktop — so "another window" was already a
/// fiction, while another instance gives each document its own Dock tile (which
/// already renders that document's viewport), its own ⌘-Tab entry, its own menu
/// bar, and its own crash domain.
///
/// The cost is that instances share nothing but the disk: no cross-document
/// undo, no shared selection, and anything written to `UserDefaults` is
/// last-writer-wins (see `EditorModel.rememberRecent`).
@MainActor
enum AppInstance {
    /// This instance's document, for the AppDelegate paths that have no view
    /// to read it from (Finder open/drop). Weak: the model outlives nothing.
    weak static var currentModel: EditorModel?

    /// Launch another copy of this app, optionally opening a document.
    ///
    /// `createsNewApplicationInstance` is the whole trick: without it the
    /// launch is a no-op that just activates the running copy.
    static func open(document url: URL? = nil) {
        let config = NSWorkspace.OpenConfiguration()
        config.createsNewApplicationInstance = true
        config.activates = true
        // Pass the document as an argument rather than an environment variable:
        // env is inherited by anything the instance spawns, and a stale
        // VCAD_OPEN two launches later would silently reopen the wrong file.
        if let url { config.arguments = [Self.openFlag, url.path] }

        let me = Bundle.main.bundleURL
        NSWorkspace.shared.openApplication(at: me, configuration: config) { _, error in
            guard let error else { return }
            // Not fatal: the current instance is still usable, and saying so is
            // better than a menu item that silently does nothing.
            FileHandle.standardError.write(
                Data("[vcad] could not open a new instance: \(error.localizedDescription)\n".utf8))
        }
    }

    /// The flag this app passes to itself. Not a user-facing CLI.
    static let openFlag = "--vcad-open"

    /// The document this instance was launched to open, if any.
    ///
    /// Accepts the launch argument, the `VCAD_OPEN` dev hook, and a bare path
    /// argument (so `open -a vcad --args file.vcad` and a drop onto the icon
    /// both work).
    static func launchDocument() -> URL? {
        let args = CommandLine.arguments
        if let i = args.firstIndex(of: openFlag), i + 1 < args.count {
            return URL(fileURLWithPath: args[i + 1])
        }
        if let path = ProcessInfo.processInfo.environment["VCAD_OPEN"], !path.isEmpty {
            return URL(fileURLWithPath: path)
        }
        if let bare = args.dropFirst().first(where: { $0.hasSuffix(".vcad") || $0.hasSuffix(".loon") }) {
            return URL(fileURLWithPath: bare)
        }
        return nil
    }

    /// Where a document should open: in this instance, or a new one.
    ///
    /// An untouched sandbox is a scratch instance — reuse it, the way a Mac app
    /// reuses its empty Untitled window. Anything else already has a document
    /// worth keeping on screen, so the new one gets its own instance.
    static func opening(_ url: URL, from model: EditorModel) {
        if model.isDisposableScratch {
            model.openDocument(url)
        } else {
            open(document: url)
        }
    }
}
