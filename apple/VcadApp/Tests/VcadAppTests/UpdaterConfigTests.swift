import XCTest
import VcadUpdate

final class UpdaterConfigTests: XCTestCase {
    private let feed = "https://vcad.io/desktop/appcast.xml"

    func testReadsFeedURL() throws {
        let c = try UpdaterConfig(info: ["SUFeedURL": feed, "SUPublicEDKey": "abc="], mode: .release)
        XCTAssertEqual(c.feedURL, URL(string: feed))
        XCTAssertEqual(c.feedURL, UpdaterConfig.defaultFeedURL)
        XCTAssertEqual(c.publicEDKey, "abc=")
        XCTAssertTrue(c.updatesEnabled)
    }

    func testReleaseRefusesEmptyKey() {
        for info: [String: Any] in [
            ["SUFeedURL": feed],
            ["SUFeedURL": feed, "SUPublicEDKey": ""],
            ["SUFeedURL": feed, "SUPublicEDKey": "  \n"],
        ] {
            XCTAssertThrowsError(try UpdaterConfig(info: info, mode: .release)) {
                XCTAssertEqual($0 as? UpdaterConfig.ConfigError, .missingPublicKey)
            }
        }
    }

    func testReleaseModeFromInfoPlistFlag() {
        let info: [String: Any] = ["SUFeedURL": feed, "SUPublicEDKey": "", "VCADReleaseBuild": true]
        XCTAssertEqual(UpdaterConfig.mode(from: info), .release)
        XCTAssertThrowsError(try UpdaterConfig(info: info))
    }

    func testDevSkipsMissingKey() throws {
        let c = try UpdaterConfig(info: ["SUFeedURL": feed], mode: .dev)
        XCTAssertNil(c.publicEDKey)
        XCTAssertFalse(c.updatesEnabled)
    }

    func testRejectsMissingOrInsecureFeed() {
        XCTAssertThrowsError(try UpdaterConfig(info: [:], mode: .dev)) {
            XCTAssertEqual($0 as? UpdaterConfig.ConfigError, .missingFeedURL)
        }
        XCTAssertThrowsError(try UpdaterConfig(info: ["SUFeedURL": "http://vcad.io/a.xml"], mode: .dev))
    }
}
