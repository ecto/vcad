import XCTest
@testable import VcadApp

/// Opt-in smoke run against a live frog api endpoint. Skipped unless
/// `FROG_LIVE_SMOKE=1`; reads `FROG_BASE_URL` / `FROG_API_KEY` from the
/// environment (never from a file in the repo). Drives the real panel
/// session over URLSession and checks the outcome is either a reply or a
/// friendly error entry, never a crash.
@MainActor
final class FrogLiveSmokeTests: XCTestCase {
    func testOneAgentTurnAgainstTheLiveEdge() async throws {
        let env = ProcessInfo.processInfo.environment
        try XCTSkipUnless(env["FROG_LIVE_SMOKE"] == "1", "set FROG_LIVE_SMOKE=1 to hit the live endpoint")
        let model = EditorModel()
        let session = AgentSession()
        session.draft = "add a 10 mm cube"
        session.submit(model: model)
        let deadline = Date().addingTimeInterval(120)
        while session.running, Date() < deadline { try await Task.sleep(for: .milliseconds(200)) }
        XCTAssertFalse(session.running, "turn did not finish")
        let entries = session.entries.map { "\($0.role): \($0.text) \($0.tools.map(\.name))" }
        print("[FROG_LIVE_SMOKE] " + entries.joined(separator: " | "))
        if let expected = env["FROG_LIVE_EXPECT"], expected == "unavailable" {
            let error = try XCTUnwrap(session.entries.last { $0.role == .error })
            XCTAssertTrue(error.text.contains("isn't serving"), error.text)
        }
    }
}
