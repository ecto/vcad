import XCTest
import simd
@testable import VcadApp

@MainActor
final class K1PerfTests: XCTestCase {
    func testK1SimThroughputHeadless() throws {
        let root = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent()
        let url = root.appendingPathComponent("examples/k1-floating.vcad")
        let doc = try Data(contentsOf: url)
        let dir = url.deletingLastPathComponent().path
        let dict = try JSONSerialization.jsonObject(with: doc) as! [String: Any]
        var spec = SimSpec.autoConfigured(for: dict)
        spec.base_dir = dir
        spec.max_steps = 100_000

        let env = try RobotEnv(documentJSON: doc, spec: spec)
        try env.reset(seed: 1)
        let hold = [Double](repeating: 0, count: env.actionDim)
        var reward = RewardSpec(); reward.nominal_height_m = spec.nominal_height_m

        // Phase A: raw step
        for _ in 0..<20 { _ = try env.step(positionTargets: hold) }
        var t = Date(); for _ in 0..<200 { _ = try env.step(positionTargets: hold) }
        let stepMs = Date().timeIntervalSince(t) / 200 * 1000

        // Phase B: step + reward (JSON round-trip per call)
        t = Date(); for _ in 0..<200 { _ = try env.step(positionTargets: hold); _ = env.reward(reward) }
        let stepRewardMs = Date().timeIntervalSince(t) / 200 * 1000

        // Phase C: + scene transforms
        let authored = [float4x4](repeating: matrix_identity_float4x4, count: env.bodyIDs.count)
        t = Date()
        for _ in 0..<200 { _ = try env.step(positionTargets: hold); _ = env.reward(reward); _ = env.sceneTransforms(fallback: authored) }
        let allMs = Date().timeIntervalSince(t) / 200 * 1000

        let budget = env.controlDt * 1000
        print(String(format: "K1 PERF  budget %.1f ms | step %.2f | +reward %.2f (reward %.2f) | +transforms %.2f  => RTF %.2fx",
                     budget, stepMs, stepRewardMs, stepRewardMs - stepMs, allMs, budget / allMs))
        XCTAssertLessThan(allMs, budget, "K1 must simulate faster than real time")
    }
}
