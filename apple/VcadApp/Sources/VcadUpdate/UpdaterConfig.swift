import Foundation

/// Auto-update configuration read from the bundle's Info.plist.
///
/// bundle.sh writes `SUFeedURL` always, `SUPublicEDKey` from the
/// `SPARKLE_PUBLIC_ED_KEY` env var, and `VCADReleaseBuild` = true for
/// Developer-ID-signed builds. The key is never hardcoded in source.
///
/// Release builds refuse a missing or empty key (Sparkle would otherwise
/// accept unsigned updates or fail opaquely). Dev builds without a key get
/// `updatesEnabled == false`: the menu item stays, checks are skipped.
public struct UpdaterConfig: Equatable, Sendable {
    public static let defaultFeedURL = URL(string: "https://vcad.io/desktop/appcast.xml")!

    public enum Mode: Equatable, Sendable { case release, dev }

    public enum ConfigError: Error, Equatable, CustomStringConvertible {
        case missingFeedURL
        case invalidFeedURL(String)
        case missingPublicKey

        public var description: String {
            switch self {
            case .missingFeedURL: return "SUFeedURL is missing from Info.plist"
            case .invalidFeedURL(let s): return "SUFeedURL is not an https URL: \(s)"
            case .missingPublicKey:
                return "SUPublicEDKey is missing or empty in a release build (set SPARKLE_PUBLIC_ED_KEY when bundling)"
            }
        }
    }

    public let feedURL: URL
    public let publicEDKey: String?
    public let mode: Mode

    /// Whether Sparkle should be started at all.
    public var updatesEnabled: Bool { publicEDKey != nil }

    public static func mode(from info: [String: Any]) -> Mode {
        (info["VCADReleaseBuild"] as? Bool) == true ? .release : .dev
    }

    public init(info: [String: Any], mode: Mode? = nil) throws {
        let mode = mode ?? Self.mode(from: info)
        guard let raw = (info["SUFeedURL"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines),
              !raw.isEmpty else { throw ConfigError.missingFeedURL }
        guard let url = URL(string: raw), url.scheme == "https", url.host != nil else {
            throw ConfigError.invalidFeedURL(raw)
        }
        let key = (info["SUPublicEDKey"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines)
        let usableKey = (key?.isEmpty == false) ? key : nil
        if mode == .release && usableKey == nil { throw ConfigError.missingPublicKey }
        self.feedURL = url
        self.publicEDKey = usableKey
        self.mode = mode
    }
}
