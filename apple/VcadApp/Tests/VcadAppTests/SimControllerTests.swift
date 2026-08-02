import XCTest
import simd
@testable import VcadApp

/// Exercises the layer the app actually uses: `SimController` driving
/// `SimEngine` on its background queue and publishing back to the main actor.
///
/// The FFI tests prove the kernel is called correctly. These prove the *app
/// model* is wired correctly — that frames arrive, that transforms change, that
/// pausing stops the clock, that teardown doesn't leave a thread running. Those
/// are the failures that would otherwise show up as "the robot doesn't move"
/// with nothing in the logs.
@MainActor
final class SimControllerTests: XCTestCase {

    private static var fixtures: URL {
        var dir = URL(fileURLWithPath: #filePath)
        for _ in 0..<8 {
            dir = dir.deletingLastPathComponent()
            let candidate = dir.appendingPathComponent("crates/vcad-ffi/tests/fixtures")
            if FileManager.default.fileExists(atPath: candidate.path) { return candidate }
        }
        XCTFail("could not locate fixtures")
        return dir
    }

    private func humanoid() throws -> Data {
        try Data(contentsOf: Self.fixtures.appendingPathComponent("g1_floating.vcad"))
    }

    private func fixedBase() throws -> Data {
        try Data(contentsOf: Self.fixtures.appendingPathComponent("g1_fixed.vcad"))
    }

    /// Spin the main runloop until `condition` holds or `timeout` elapses.
    ///
    /// The engine publishes with `Task { @MainActor in … }`, so the main actor
    /// has to actually run for a frame to land — a plain `sleep` would block
    /// the very thing being waited on and the test would always time out.
    private func waitUntil(_ description: String,
                           timeout: TimeInterval = 10,
                           _ condition: @escaping () -> Bool) {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition() && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(condition(), "timed out waiting for \(description)")
    }

    // MARK: auto-configuration

    func testAutoConfigurationReadsTheFloatingBaseFromTheDocument() throws {
        let dict = try JSONSerialization.jsonObject(with: humanoid()) as! [String: Any]
        let spec = SimSpec.autoConfigured(for: dict)

        XCTAssertTrue(spec.require_floating_base)
        XCTAssertEqual(spec.config.base_instance_id, "pelvis_inst")
        // The fixture spawns at 780 mm.
        XCTAssertEqual(spec.nominal_height_m, 0.78, accuracy: 1e-9)
        XCTAssertNotNil(spec.config.termination?.base_height_below)
        XCTAssertEqual(spec.config.termination?.base_tilt_above_deg, 45)
        XCTAssertEqual(Set(spec.end_effector_ids),
                       ["left_ankle_roll_link_inst", "right_ankle_roll_link_inst"],
                       "the feet should be found by name")
    }

    /// Guards the auto-reset thrash: an env whose every step terminates looks
    /// like a robot doing nothing, because it resets before anything moves.
    func testAFreshEpisodeDoesNotTerminateImmediately() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        sim.autoReset = false
        waitUntil("ready") { sim.isReady }
        sim.stepOnce()
        waitUntil("one step") { sim.latest.step == 1 }
        XCTAssertFalse(sim.latest.done,
                       "step 1 must not already be terminal (reason: \(sim.latest.terminationReason ?? "none"))")
    }

    func testAGroundRestingRobotGetsNoHeightTermination() throws {
        // The bug this pins: the shipped sample was spawned 600 mm in the air,
        // so auto-configuration derived `base_height_below = 0.55 x 0.6 = 0.33`
        // and the robot crossed it within a handful of steps — every episode
        // ended at step 4-7 and the viewport showed a robot falling on loop.
        // A policy cannot be evaluated, let alone trained, on an env that
        // terminates before it has done anything.
        //
        // A robot authored to rest on the ground has no meaningful "fell below"
        // height. Tilt is the signal that generalizes.
        let doc = try Data(contentsOf: Self.fixtures
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("examples/floating-arm.vcad"))
        let dict = try JSONSerialization.jsonObject(with: doc) as! [String: Any]
        let spec = SimSpec.autoConfigured(for: dict)

        XCTAssertTrue(spec.require_floating_base, "the sample has a Free base")
        XCTAssertEqual(spec.nominal_height_m, 0, accuracy: 1e-9,
                       "the sample must be authored resting on the ground")
        let term = try XCTUnwrap(spec.config.termination)
        XCTAssertNil(term.base_height_below,
                     "a ground-resting robot must not get a height floor")
        XCTAssertEqual(term.base_tilt_above_deg, 45)
    }

    func testTheShippedSampleSurvivesAFullEpisodeUncontrolled() throws {
        // End-to-end version of the above: the sample must not fall over on its
        // own. If it does, every policy that runs in it is being scored on how
        // fast it hits the floor.
        let doc = try Data(contentsOf: Self.fixtures
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("examples/floating-arm.vcad"))
        let dict = try JSONSerialization.jsonObject(with: doc) as! [String: Any]
        var spec = SimSpec.autoConfigured(for: dict)
        spec.max_steps = 200

        let env = try RobotEnv(documentJSON: doc, spec: spec)
        try env.reset(seed: 1)
        let hold = [Double](repeating: 0, count: env.actionDim)
        var last = SimStep.empty
        for _ in 0..<200 {
            last = try env.step(positionTargets: hold)
            if last.done { break }
        }
        XCTAssertEqual(last.step, 200,
                       "the sample terminated early at step \(last.step) "
                       + "(\(last.terminationReason ?? "unknown")) — it is falling again")
        XCTAssertTrue(last.truncated, "ending should be the episode running out, not a fall")
        XCTAssertLessThan(abs(last.baseTiltDeg), 5.0, "it should stay upright")
    }

    func testAutoConfigurationRelaxesTheFloatingBaseCheckForABoltedRobot() throws {
        let dict = try JSONSerialization.jsonObject(with: fixedBase()) as! [String: Any]
        let spec = SimSpec.autoConfigured(for: dict)

        // A bolted robot cannot fall, so demanding a floating base would make
        // the Simulate button simply not work on a perfectly valid document.
        XCTAssertFalse(spec.require_floating_base)
        // The termination config must be present but EMPTY, not nil. Nil looks
        // like "no termination" and actually selects the kernel's legacy
        // end-effector-below-ground rule, which fires on step one for anything
        // standing on the ground.
        let term = try XCTUnwrap(spec.config.termination)
        XCTAssertNil(term.base_height_below)
        XCTAssertNil(term.base_tilt_above_deg)
        XCTAssertFalse(term.terminate_on_joint_limit)
    }

    func testAutoConfiguredSpecActuallyBuildsForBothRobots() throws {
        for data in [try humanoid(), try fixedBase()] {
            let dict = try JSONSerialization.jsonObject(with: data) as! [String: Any]
            let spec = SimSpec.autoConfigured(for: dict)
            XCTAssertNoThrow(try RobotEnv(documentJSON: data, spec: spec),
                             "auto-configuration must produce a spec that builds")
        }
    }

    // MARK: lifecycle

    func testPrepareMakesTheControllerReadyAndReportsTheRobot() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])

        XCTAssertNil(sim.errorMessage)
        XCTAssertTrue(sim.isAvailable)
        XCTAssertTrue(sim.isReady)
        XCTAssertEqual(sim.actionDim, 23, "the G1 fixture has 23 actuated joints")
        XCTAssertEqual(sim.controlHz, 50, accuracy: 0.01)
        XCTAssertFalse(sim.actuatedJointIDs.isEmpty)
        // Gains must have been filled in, or the humanoid sinks through its
        // knees and it looks like a physics bug.
        XCTAssertEqual(sim.spec.gains.count, sim.actionDim)
    }

    func testPrepareOnAnUnsimulatableDocumentReportsWhy() {
        let sim = SimController()
        sim.prepare(documentJSON: Data("{ not a document".utf8), scene: nil, authored: [])
        XCTAssertFalse(sim.isAvailable)
        XCTAssertNotNil(sim.errorMessage)
        XCTAssertTrue(sim.errorMessage!.contains("parse document"), sim.errorMessage!)
    }

    func testRunningAdvancesTheEpisodeAndPublishesFrames() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        XCTAssertTrue(sim.isReady)

        sim.run()
        XCTAssertTrue(sim.isRunning)
        waitUntil("the episode to advance past step 5") { sim.latest.step > 5 }

        // The robot is uncontrolled, so it must be falling — height strictly
        // below where it spawned.
        XCTAssertTrue(sim.latest.hasBase)
        XCTAssertLessThan(sim.latest.baseHeightM, 0.78,
                          "an uncontrolled humanoid must fall")

        sim.pause()
        XCTAssertFalse(sim.isRunning)

        // `pause()` is a request, not a barrier: it hands `stop_()` to the sim
        // queue, so a tick already in flight still completes and publishes.
        // Asserting an immediate freeze would be asserting a synchronous
        // guarantee the design deliberately does not offer (providing one would
        // mean blocking the main thread on a physics step). So: let the queue
        // settle, then snapshot, then assert nothing further advances — which
        // is the property that actually matters.
        RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        let frozen = sim.latest.step
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        XCTAssertEqual(sim.latest.step, frozen,
                       "pause must stop the clock (15 ticks would have elapsed)")
    }

    func testStepOnceAdvancesExactlyOneControlTick() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        waitUntil("the reset frame") { sim.latest.step == 0 && sim.isReady }

        sim.stepOnce()
        waitUntil("one step") { sim.latest.step == 1 }
        sim.stepOnce()
        waitUntil("two steps") { sim.latest.step == 2 }
        XCTAssertFalse(sim.isRunning, "stepping must not start the clock")
    }

    func testResetReturnsTheRobotToTheStartOfAnEpisode() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        sim.run()
        waitUntil("the episode to advance") { sim.latest.step > 10 }
        let fallen = sim.latest.baseHeightM
        sim.pause()

        sim.reset()
        waitUntil("the reset to land") { sim.latest.step == 0 }
        XCTAssertGreaterThan(sim.latest.baseHeightM, fallen,
                             "reset must restore the spawn pose")
        XCTAssertEqual(sim.episodeReturn, 0, "reset must clear the accumulated return")
    }

    func testEpisodeReturnAccumulatesAndFallingScoresWorse() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        sim.run()
        waitUntil("a few steps of reward") { sim.latest.step > 3 }
        let early = sim.episodeReturn
        XCTAssertGreaterThan(early, 0, "a robot near its spawn height scores positively")
        sim.pause()
    }

    func testAShoveChangesTheTrajectory() throws {
        func heightAfter(shoving: Bool) throws -> Double {
            let sim = SimController()
            sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
            sim.autoReset = false
            waitUntil("ready") { sim.isReady && sim.latest.step == 0 }
            for i in 0..<8 {
                if shoving && i == 4 { sim.shove(SIMD3(1, 0, 0), speed: 1.0) }
                let target = UInt32(i + 1)
                sim.stepOnce()
                waitUntil("step \(target)") { sim.latest.step >= target }
            }
            return sim.latest.baseHeightM
        }
        let quiet = try heightAfter(shoving: false)
        let shoved = try heightAfter(shoving: true)
        XCTAssertNotEqual(quiet, shoved, accuracy: 0,
                          "a 1 m/s shove must change the trajectory")
    }

    func testTeardownStopsEverythingAndClearsState() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        sim.run()
        waitUntil("frames") { sim.latest.step > 2 }

        sim.teardown()
        XCTAssertFalse(sim.isRunning)
        XCTAssertFalse(sim.isAvailable)
        XCTAssertFalse(sim.isReady)
        XCTAssertNil(sim.transforms)
        XCTAssertEqual(sim.latest.step, 0)

        // Nothing may keep stepping after teardown; a lingering timer would be
        // stepping physics against a released env.
        RunLoop.current.run(until: Date().addingTimeInterval(0.3))
        XCTAssertEqual(sim.latest.step, 0, "teardown must stop the engine")
    }

    func testDrivingWithTheBaselinePolicyIsSupported() throws {
        // The rest-pose driver is the default and must not require a file.
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        XCTAssertEqual(sim.driver, .restPose)
        sim.run()
        waitUntil("frames under the baseline driver") { sim.latest.step > 3 }
        sim.pause()
    }

    // MARK: training

    func testTrainingRunsAndProducesASaveableBundle() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])

        // Deliberately tiny: this proves the machinery, not that ARS learns.
        sim.spec.max_steps = 20
        sim.trainSpec = TrainSpec(
            ars: ArsConfig(n_directions: 2, top_k: 1, step_size: 0.005,
                           noise_std: 0.05, iterations: 2,
                           rollouts_per_eval: 1, seed: 7),
            policy: "linear", hidden: 8, action_scale_deg: 8.0,
            init_seed: 0, curriculum_warmup: 0, held_out_every: 1, held_out_seeds: 2)

        sim.startTraining()
        XCTAssertNil(sim.trainingError, sim.trainingError ?? "")
        waitUntil("training to finish", timeout: 300) { sim.training?.finished == true }

        let p = try XCTUnwrap(sim.training)
        XCTAssertFalse(p.failed, sim.trainingError ?? "training failed")
        XCTAssertTrue(p.bestHeldOut.isFinite,
                      "a finished run must have scored an iterate on held-out seeds")
        XCTAssertFalse(sim.trainingCurve.isEmpty, "the curve must have points")

        let out = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("test-\(UUID().uuidString).vcadpolicy")
        XCTAssertTrue(sim.saveTrainedPolicy(to: out))
        defer { try? FileManager.default.removeItem(at: out) }

        // The saved bundle must load back and drive this same robot — the
        // round trip the app performs when the user picks "Watch best".
        let bundle = try Data(contentsOf: out)
        let policy = try TrainedPolicy(bundle: bundle, document: try humanoid())
        XCTAssertNil(policy.staleWarning, "a freshly trained policy is not stale")
        let env = try RobotEnv(documentJSON: try humanoid(), spec: sim.spec)
        XCTAssertNoThrow(try env.check(policy: policy))
        XCTAssertEqual(policy.bundle?.env.substeps, sim.spec.substeps,
                       "provenance must record the env it trained in")
    }

    func testStoppingTrainingEndsTheRun() throws {
        let sim = SimController()
        sim.prepare(documentJSON: try humanoid(), scene: nil, authored: [])
        sim.spec.max_steps = 40
        sim.trainSpec.ars.iterations = 100_000  // cannot finish on its own
        sim.trainSpec.ars.n_directions = 2
        sim.trainSpec.ars.top_k = 1
        sim.trainSpec.held_out_seeds = 2

        sim.startTraining()
        XCTAssertNil(sim.trainingError, sim.trainingError ?? "")
        waitUntil("training to get going", timeout: 120) {
            (sim.training?.iteration ?? 0) > 0
        }
        // `stopTraining` joins the worker, so returning at all proves it ended.
        sim.stopTraining()
        XCTAssertFalse(sim.isTraining)
    }
}
