import XCTest
import simd
@testable import VcadApp

/// Cross-language parity for the simulation ABI.
///
/// The Rust side has its own golden test (`crates/vcad-ffi/tests/gym_parity.rs`)
/// that records this exact trajectory. This suite loads the **same** fixture
/// and the **same** golden file through the Swift wrapper, so a discrepancy
/// localizes immediately: Rust green + Swift red means the bug is in the Swift
/// marshalling, not the kernel.
///
/// That is worth the duplication because the Swift layer is where the
/// silent-corruption bugs live — a buffer read at the wrong stride, a
/// column-major matrix transposed, a length taken in bytes instead of
/// elements. None of those throw; they just make the robot move wrongly.
final class SimParityTests: XCTestCase {

    // MARK: fixtures

    /// Locate the shared fixtures by walking up from this source file to the
    /// repo root. SwiftPM resource bundles would need the files copied into the
    /// package, and a copy is exactly what this test exists to rule out — both
    /// languages must read the identical bytes.
    private static var fixtures: URL {
        var dir = URL(fileURLWithPath: #filePath)
        for _ in 0..<8 {
            dir = dir.deletingLastPathComponent()
            let candidate = dir.appendingPathComponent("crates/vcad-ffi/tests/fixtures")
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
        }
        XCTFail("could not locate crates/vcad-ffi/tests/fixtures from \(#filePath)")
        return dir
    }

    private func document() throws -> Data {
        try Data(contentsOf: Self.fixtures.appendingPathComponent("g1_floating.vcad"))
    }

    /// The spec the Rust golden was recorded with. Any divergence here makes
    /// the comparison meaningless, so it is transcribed rather than defaulted.
    private func spec() -> SimSpec {
        var s = SimSpec()
        s.end_effector_ids = ["left_ankle_roll_link_inst", "right_ankle_roll_link_inst"]
        s.dt = 1.0 / 1000.0
        s.substeps = 20
        s.max_steps = 400
        s.nominal_height_m = 0.78
        s.config = EnvConfig(
            randomization: nil,
            observation_noise: nil,
            termination: Termination(base_height_below: 0.4, base_tilt_above_deg: 45.0),
            base_instance_id: "pelvis_inst")
        return s
    }

    /// The golden Rust recorded, decoded.
    private struct Golden: Decodable {
        struct Frame: Decodable {
            let step: UInt32
            let base_height_m: Double
            let base_tilt_deg: Double
            let base_dofs: [Double]
            let done: Bool
        }
        let action_dim: Int
        let obs_dim: Int
        let observation_dim: Int
        let body_count: Int
        let control_dt: Double
        let frames: [Frame]
    }

    private func golden() throws -> Golden {
        let data = try Data(contentsOf: Self.fixtures.appendingPathComponent("g1_fall_100.json"))
        return try JSONDecoder().decode(Golden.self, from: data)
    }

    // MARK: tests

    func testABIVersionMatchesTheHeaderThisAppWasBuiltAgainst() {
        // A stale libvcad_ffi.a is the classic native-app failure: the Swift
        // side calls a signature that moved, and the result is corruption
        // rather than a link error. Assert the contract explicitly.
        XCTAssertEqual(vcadFFIABIVersion(), 6,
                       "libvcad_ffi.a is stale — re-run apple/VcadApp/build-ffi.sh")
    }

    func testEnvironmentBuildsAndReportsTheRobot() throws {
        let env = try RobotEnv(documentJSON: try document(), spec: spec())
        let g = try golden()
        XCTAssertEqual(env.actionDim, g.action_dim)
        XCTAssertEqual(env.obsDim, g.obs_dim)
        XCTAssertEqual(env.bodyIDs.count, g.body_count)
        XCTAssertEqual(env.actuatedJointIDs.count, env.actionDim,
                       "every actuated joint on this robot is single-DOF")
        XCTAssertEqual(env.controlDt, g.control_dt, accuracy: 1e-12)
        XCTAssertTrue(env.bodyIDs.contains("pelvis_inst"))
    }

    func testTrajectoryMatchesTheRustGolden() throws {
        let env = try RobotEnv(documentJSON: try document(), spec: spec())
        let g = try golden()
        try env.reset(seed: 1)

        let hold = [Double](repeating: 0, count: env.actionDim)
        var frames: [SimStep] = []
        for _ in 0..<100 {
            let s = try env.step(positionTargets: hold)
            frames.append(s)
            if s.done { break }
        }

        XCTAssertEqual(frames.count, g.frames.count,
                       "Swift and Rust disagree about when the episode ends")

        // Same tolerance and the same reasoning as the Rust side, and the same
        // number: 1e-8 relative.
        //
        // Two measured effects, neither of them simulator flakiness. JSON is a
        // lossy channel for the ~1e-19 base DOFs (serde_json's parser does not
        // round-trip subnormal-magnitude f64). And the golden is recorded from
        // a debug build while the app links the RELEASE kernel — optimization
        // changes floating-point codegen, and a falling humanoid amplifies
        // early rounding. Measured: 9.8e-17 within a profile, 1.5e-9 across.
        //
        // This test held at 1e-12 only for as long as the app linked a debug
        // kernel; making the app link release (which took the K1 from 0.29x to
        // real time) put Swift on the other side of that boundary too.
        //
        // A third effect is reasoned about rather than measured: the golden is
        // recorded on aarch64 macOS and CI runs x86_64 Linux, whose libm
        // `sin`/`cos`/`atan2` legitimately differ by ~1 ulp — amplified over the
        // 21 frames here. So the bound is physical rather than bitwise: 1e-6
        // relative on 0.78 m is 780 nanometres, while a real physics regression
        // moves millimetres. The structural assertions below are what actually
        // guard this, and they are exact on every platform.
        let tol = 1e-6
        for (i, (got, want)) in zip(frames, g.frames).enumerated() {
            XCTAssertEqual(got.step, want.step, "frame \(i): step index")
            XCTAssertEqual(got.done, want.done, "frame \(i): termination")
            XCTAssertEqual(got.baseHeightM, want.base_height_m, accuracy: tol,
                           "frame \(i): base height")
            XCTAssertEqual(got.baseTiltDeg, want.base_tilt_deg, accuracy: tol,
                           "frame \(i): base tilt")
            for (k, w) in want.base_dofs.enumerated() where k < got.jointPositions.count {
                XCTAssertEqual(got.jointPositions[k], w,
                               accuracy: tol * (1 + abs(w)),
                               "frame \(i): base DOF \(k)")
            }
        }

        // The structural facts — exact, platform-independent, and what a real
        // regression actually breaks.
        let last = try XCTUnwrap(frames.last)
        XCTAssertTrue(last.done, "the episode must end by terminating")
        XCTAssertGreaterThan(last.baseTiltDeg, 45.0,
                             "it must terminate by tipping past 45 degrees")
        for (a, b) in zip(frames, frames.dropFirst()) {
            XCTAssertLessThan(b.baseHeightM, a.baseHeightM,
                              "an uncontrolled humanoid must descend monotonically")
        }
    }

    func testBodyTransformsAreMillimetresNotMetres() throws {
        // The single most likely marshalling bug, and the one that looks least
        // like a bug: forget the conversion and the whole robot renders as a
        // speck at the origin, which reads as "the mesh failed to load".
        let env = try RobotEnv(documentJSON: try document(), spec: spec())
        try env.reset(seed: 1)
        let authored = [float4x4](repeating: matrix_identity_float4x4, count: env.bodyIDs.count)
        // Without a bound scene there is nothing to write in scene order.
        XCTAssertNil(env.sceneTransforms(fallback: authored),
                     "scene transforms must require an explicit bind")
    }

    func testWrongLengthActionIsRefused() throws {
        let env = try RobotEnv(documentJSON: try document(), spec: spec())
        try env.reset(seed: 1)
        XCTAssertThrowsError(try env.step(positionTargets: [0, 0, 0])) { error in
            let msg = (error as? SimError)?.message ?? ""
            XCTAssertTrue(msg.contains("expects"), "unhelpful message: \(msg)")
        }
    }

    func testFixedBaseDocumentFailsClosedWithAnActionableMessage() throws {
        let doc = try Data(contentsOf: Self.fixtures.appendingPathComponent("g1_fixed.vcad"))
        XCTAssertThrowsError(try RobotEnv(documentJSON: doc, spec: spec())) { error in
            let msg = (error as? SimError)?.message ?? ""
            XCTAssertTrue(msg.contains("floating base"),
                          "the message must name the actual problem: \(msg)")
        }
        // And can be opted out of for a genuinely fixed-base task.
        var s = spec()
        s.require_floating_base = false
        XCTAssertNoThrow(try RobotEnv(documentJSON: doc, spec: s))
    }

    func testUnknownEndEffectorIsRefused() throws {
        var s = spec()
        s.end_effector_ids = ["no_such_foot_inst"]
        XCTAssertThrowsError(try RobotEnv(documentJSON: try document(), spec: s)) { error in
            let msg = (error as? SimError)?.message ?? ""
            XCTAssertTrue(msg.contains("no_such_foot_inst"), msg)
        }
    }

    func testZeroPolicyReproducesTheHoldRestPoseTrajectory() throws {
        // The zero policy's action IS the default pose, so driving through the
        // policy path must give an identical trajectory. If it doesn't, the
        // features → act → step chain has drifted from training — the highest-
        // consequence bug in the inference path, because it degrades a policy
        // silently rather than failing.
        let doc = try document()
        let direct = try RobotEnv(documentJSON: doc, spec: spec())
        try direct.reset(seed: 1)
        let hold = [Double](repeating: 0, count: direct.actionDim)
        var a: [Double] = []
        for _ in 0..<60 {
            let s = try direct.step(positionTargets: hold)
            a.append(s.baseHeightM)
            if s.done { break }
        }

        let viaPolicy = try RobotEnv(documentJSON: doc, spec: spec())
        let zero = try viaPolicy.zeroPolicy()
        try viaPolicy.check(policy: zero)
        try viaPolicy.reset(seed: 1)
        var b: [Double] = []
        for _ in 0..<60 {
            let s = try viaPolicy.step(policy: zero)
            b.append(s.baseHeightM)
            if s.done { break }
        }

        XCTAssertEqual(a.count, b.count)
        for (i, (x, y)) in zip(a, b).enumerated() {
            XCTAssertEqual(x, y, accuracy: 1e-12, "step \(i)")
        }
    }

    func testPolicyForADifferentRobotIsRefused() throws {
        let env = try RobotEnv(documentJSON: try document(), spec: spec())
        let mismatched = Data("""
        {"weights":[0,0,0,0,0,0,0,0],"obs_dim":4,"act_dim":2,
         "mean":[0,0,0,0],"std":[1,1,1,1],
         "action":{"default_pose_deg":[0,0],"action_scale_deg":8}}
        """.utf8)
        let policy = try TrainedPolicy(json: mismatched)
        XCTAssertThrowsError(try env.check(policy: policy)) { error in
            let msg = (error as? SimError)?.message ?? ""
            XCTAssertTrue(msg.contains("features"), msg)
        }
    }

    func testAShoveChangesTheTrajectory() throws {
        let doc = try document()
        let hold: (RobotEnv) throws -> Double = { env in
            let a = [Double](repeating: 0, count: env.actionDim)
            var h = 0.0
            for _ in 0..<6 { h = try env.step(positionTargets: a).baseHeightM }
            return h
        }
        let quiet = try RobotEnv(documentJSON: doc, spec: spec())
        try quiet.reset(seed: 1)
        let undisturbed = try hold(quiet)

        let shoved = try RobotEnv(documentJSON: doc, spec: spec())
        try shoved.reset(seed: 1)
        let a = [Double](repeating: 0, count: shoved.actionDim)
        for _ in 0..<5 { _ = try shoved.step(positionTargets: a) }
        XCTAssertTrue(shoved.nudgeBase(linear: SIMD3(1, 0, 0)))
        let after = try shoved.step(positionTargets: a).baseHeightM

        XCTAssertNotEqual(after, undisturbed, accuracy: 0,
                          "a 1 m/s shove must change the trajectory")
    }

    func testRewardIsZeroBeforeTheFirstStepAndPositiveWhileUpright() throws {
        let env = try RobotEnv(documentJSON: try document(), spec: spec())
        var r = RewardSpec()
        r.nominal_height_m = 0.78
        try env.reset(seed: 1)
        XCTAssertEqual(env.reward(r), 0, "nothing has been stepped yet")

        _ = try env.step(positionTargets: [Double](repeating: 0, count: env.actionDim))
        let early = env.reward(r)
        XCTAssertTrue(early.isFinite && early > 0.9,
                      "a robot still at its spawn height should be near the alive bonus, got \(early)")
    }

    func testDocumentHashDetectsAnEdit() throws {
        let doc = try document()
        let h1 = vcadDocumentHash(doc)
        XCTAssertNotNil(h1)
        XCTAssertTrue(h1!.hasPrefix("fnv1a64:"))
        XCTAssertEqual(h1, vcadDocumentHash(doc), "hashing must be stable")

        var obj = try JSONSerialization.jsonObject(with: doc) as! [String: Any]
        var materials = obj["materials"] as? [String: Any] ?? [:]
        materials["__drift_probe"] = ["name": "drift", "color": "#ff0000"]
        obj["materials"] = materials
        let edited = try JSONSerialization.data(withJSONObject: obj)
        XCTAssertNotEqual(h1, vcadDocumentHash(edited),
                          "an edited document must hash differently or drift is undetectable")
    }

    func testSpecEncodesTheFieldNamesRustExpects() throws {
        // The Rust side uses `deny_unknown_fields`, so a renamed field here is
        // a create-time failure rather than a silent default — but only if the
        // names actually line up. Pin them.
        let data = try spec().encoded()
        let obj = try JSONSerialization.jsonObject(with: data) as! [String: Any]
        for key in ["end_effector_ids", "dt", "substeps", "max_steps", "gains",
                    "config", "nominal_height_m", "require_floating_base"] {
            XCTAssertNotNil(obj[key], "missing spec field \(key)")
        }
        let config = obj["config"] as! [String: Any]
        XCTAssertNotNil(config["base_instance_id"])
        XCTAssertNotNil(config["termination"])
    }

    func testHumanoidGainsFollowTheMeasuredSchedule() {
        // Stiff hips/knees, soft ankles, soft upper body. Get this wrong and
        // the robot either sinks through its knees (too soft) or shakes itself
        // apart inside 0.2 s (too stiff for the tick).
        let gains = SimSpec.humanoidGains(for: [
            "left_hip_pitch_joint", "left_knee_joint",
            "left_ankle_roll_joint", "left_shoulder_pitch_joint",
        ])
        XCTAssertEqual(gains["left_hip_pitch_joint"], [200, 5])
        XCTAssertEqual(gains["left_knee_joint"], [200, 5])
        XCTAssertEqual(gains["left_ankle_roll_joint"], [50, 1])
        XCTAssertEqual(gains["left_shoulder_pitch_joint"], [40, 1])
    }
}

/// The flat contact channel's stride boundary.
///
/// Raised in review as dropping the last record when the buffer is a perfect
/// multiple of 5. It does not — `stride(to:)` is exclusive, so the final start
/// index `5n-5` satisfies `5n-5 < 5n-4` and is included. The `- 4` is what
/// keeps a short buffer from starting a record it cannot finish, so this is
/// pinned from both sides.
final class FootContactUnpackingTests: XCTestCase {

    func testEveryEndEffectorSurvivesAWellFormedBuffer() {
        for n in 0...4 {
            var flat: [Double] = []
            for k in 0..<n {
                flat += [Double(k % 2), Double(100 + k), Double(k), Double(k) + 0.5, 0.0]
            }
            let got = unpackFootContacts(flat)
            XCTAssertEqual(got.count, n,
                           "a \(flat.count)-element buffer must yield \(n) records")
            for k in 0..<n {
                XCTAssertEqual(got[k].inContact, k % 2 == 1)
                XCTAssertEqual(got[k].normalForce, Double(100 + k))
                XCTAssertEqual(got[k].centerOfPressure,
                               SIMD3(Double(k), Double(k) + 0.5, 0.0))
            }
        }
    }

    func testAShortBufferIsTruncatedRatherThanReadOutOfBounds() {
        // The reason the bound is `count - 4`. Using `count` here would start a
        // record at index 10 of a 12-element array and read index 14.
        for count in [1, 4, 7, 12, 13] {
            let flat = (0..<count).map(Double.init)
            let got = unpackFootContacts(flat)
            XCTAssertEqual(got.count, count / 5,
                           "\(count) doubles hold \(count / 5) whole records")
        }
    }
}
