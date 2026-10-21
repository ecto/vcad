import XCTest
@testable import VcadApp

/// Live end-to-end: the real FrogAgent + DocumentEngine against frog api.
/// Runs only with `FROG_LIVE=1` (reads `FROG_BASE_URL` / `FROG_API_KEY` from
/// the environment, never from a file in the repo) and only while the
/// endpoint reports `serving: true`; skipped otherwise.
@MainActor
final class FrogLiveE2ETests: XCTestCase {
    func testCreateMoveInspectCube() async throws {
        let env = ProcessInfo.processInfo.environment
        try XCTSkipUnless(env["FROG_LIVE"] == "1", "set FROG_LIVE=1 to run against frog api")
        let config = FrogConfig.load(defaults: UserDefaults(suiteName: "frog-live-\(UUID())")!, env: env,
                                     keychain: FrogKeychain(service: "io.vcad.agent.live-test", account: "none"))
        try XCTSkipIf(config.apiKey == nil, "FROG_API_KEY not set")
        let serving = await Self.serving(config)
        try XCTSkipUnless(serving, "frog api is not serving (outside posted hours)")

        let engine = DocumentEngine(replicaId: 1)
        let agent = FrogAgent(client: FrogClient(config: config), manifest: .load(),
                              executor: FrogToolExecutor(engine: engine))
        var log: [String] = []
        try await agent.send("Create a 20mm cube, move it +10mm in Z, then inspect it.") { event in
            switch event {
            case .text(let t): log.append("text:\(t)")
            case let .toolStarted(_, name, args): log.append("call \(name) \(args)")
            case let .toolFinished(_, o): log.append("-> \(o.ok ? "ok" : "ERR") \(o.content.prefix(160))")
            }
        }
        print("[FROG_LIVE] " + log.joined(separator: "\n[FROG_LIVE] "))

        let state = try engine.state()
        XCTAssertEqual(state.partCount, 1, "one part")
        let scene = try engine.agentRead("describe_scene", [:])
        print("[FROG_LIVE] scene: \(scene)")
        let (zMin, zMax) = try XCTUnwrap(Self.zRange(scene), scene)
        XCTAssertEqual(zMin, 10, accuracy: 1e-6)
        XCTAssertEqual(zMax, 30, accuracy: 1e-6)
    }

    /// `GET <origin>/status?format=json` → `serving`.
    static func serving(_ config: FrogConfig) async -> Bool {
        guard var c = URLComponents(string: config.baseURL) else { return false }
        c.path = "/status"; c.query = "format=json"
        guard let url = c.url else { return false }
        var req = URLRequest(url: url)
        req.setValue(FrogClient.userAgent, forHTTPHeaderField: "user-agent")
        guard let (data, _) = try? await URLSession.shared.data(for: req),
              let obj = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return false }
        return obj["serving"] as? Bool == true
    }

    /// z of `bbox min (x, y, z) max (x, y, z)` in a describe_scene line.
    static func zRange(_ scene: String) -> (Double, Double)? {
        let nums = { (s: Substring) -> [Double] in
            s.split(whereSeparator: { "(), ".contains($0) }).compactMap { Double($0) }
        }
        guard let minR = scene.range(of: "bbox min "), let maxR = scene.range(of: " max ", range: minR.upperBound..<scene.endIndex)
        else { return nil }
        let lo = nums(scene[minR.upperBound..<maxR.lowerBound])
        let hi = nums(scene[maxR.upperBound...].prefix { $0 != "\n" })
        guard lo.count >= 3, hi.count >= 3 else { return nil }
        return (lo[2], hi[2])
    }
}
