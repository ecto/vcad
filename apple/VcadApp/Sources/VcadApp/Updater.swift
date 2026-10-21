import SwiftUI
import Sparkle
import VcadUpdate

/// Owns Sparkle's updater. Started only when `UpdaterConfig` says the bundle
/// carries a usable EdDSA key; a dev build from bundle.sh without
/// SPARKLE_PUBLIC_ED_KEY (or a bare `swift run`) keeps the menu item but
/// never contacts the feed. Automatic checks are on via Info.plist
/// (SUEnableAutomaticChecks), so Sparkle schedules them itself.
@MainActor
final class AppUpdater: ObservableObject {
    static let shared = AppUpdater()

    let config: Result<UpdaterConfig, Error>
    private let controller: SPUStandardUpdaterController?
    @Published private(set) var canCheck = false

    private init() {
        let config = Result { try UpdaterConfig(info: Bundle.main.infoDictionary ?? [:]) }
        self.config = config
        switch config {
        case .success(let c) where c.updatesEnabled:
            let controller = SPUStandardUpdaterController(
                startingUpdater: true, updaterDelegate: nil, userDriverDelegate: nil)
            self.controller = controller
            controller.updater.publisher(for: \.canCheckForUpdates).assign(to: &$canCheck)
        case .success:
            self.controller = nil
        case .failure(let error):
            // A release bundle without a key: refuse to run the updater rather
            // than accept unverifiable updates. bundle.sh already fails before
            // producing such a bundle; this is the belt to that brace.
            NSLog("vcad updater disabled: \(error)")
            self.controller = nil
        }
    }

    func checkForUpdates() { controller?.checkForUpdates(nil) }
}

/// "Check for Updates…" in the app menu, under About.
struct UpdateCommands: Commands {
    @ObservedObject var updater = AppUpdater.shared

    var body: some Commands {
        CommandGroup(after: .appInfo) {
            Button("Check for Updates…") { updater.checkForUpdates() }
                .disabled(!updater.canCheck)
        }
    }
}
