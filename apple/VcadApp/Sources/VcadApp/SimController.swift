import Foundation
import Observation
import simd
import CVcadFFI

// The app-facing simulation layer: a background stepping engine plus the
// observable model the UI binds to.
//
// Why a dedicated queue rather than the existing playback `Timer`. Kinematic
// playback is a table lookup plus one FK solve — cheap enough to run inline on
// the main thread. A physics step is not: one control step of a 23-DOF
// humanoid is 20 physics substeps, and running that on the main thread means a
// busy UI frame *slows the simulation down*. The robot then falls differently
// depending on whether a menu was open, which is indistinguishable from a
// physics bug and impossible to reproduce.
//
// So: the env lives on `SimEngine.queue` and is touched nowhere else. The
// engine publishes value types (`[float4x4]`, `SimStep`) back to the main
// actor, where the renderer consumes them exactly as it consumes FK output.

/// What is driving the robot.
enum SimDriver: Equatable {
    /// Hold the rest pose — zero position targets. The baseline.
    case restPose
    /// A loaded policy.
    case policy(name: String)

    var label: String {
        switch self {
        case .restPose: return "Rest pose"
        case .policy(let name): return name
        }
    }
}

/// Steps a `RobotEnv` on a private serial queue at the env's own control rate.
///
/// Not an `actor`: the payload is a non-`Sendable` raw pointer, and a serial
/// `DispatchQueue` expresses "this pointer is only ever touched here" more
/// directly than actor isolation can without making `RobotEnv` `Sendable` (a
/// claim that would simply be false).
final class SimEngine {
    /// The one queue the env pointer may be touched from.
    private let queue = DispatchQueue(label: "io.vcad.sim", qos: .userInitiated)
    private var timer: DispatchSourceTimer?

    /// Guarded by `queue`.
    private var env: RobotEnv?
    private var policy: TrainedPolicy?
    private var authoredTransforms: [float4x4] = []
    private var rewardSpec = RewardSpec()
    private var episodeReturn: Double = 0
    private var seed: UInt64 = 1
    private var autoResetOnDone = true

    /// Real-time-factor accounting, guarded by `queue`.
    private var stepsSinceSample = 0
    private var lastSampleTime = DispatchTime.now()

    /// Main-actor sinks for published frames and errors.
    ///
    /// Confined to `queue` like everything else here, and installed through
    /// `setSinks` rather than exposed as plain `var`s. A bare property would be
    /// written from the main actor while the timer handler reads it on the sim
    /// queue — a data race on the very field whose job is to cross actors
    /// safely. `@Sendable` because the closures themselves get handed across.
    private var onFrame: (@MainActor @Sendable ([float4x4]?, SimStep, Double, Double) -> Void)?
    private var onError: (@MainActor @Sendable (String) -> Void)?

    func setSinks(
        frame: @escaping @MainActor @Sendable ([float4x4]?, SimStep, Double, Double) -> Void,
        error: @escaping @MainActor @Sendable (String) -> Void
    ) {
        queue.async { [self] in
            onFrame = frame
            onError = error
        }
    }

    /// Hand `msg` to the error sink on the main actor.
    private func report(_ msg: String) {
        guard let sink = onError else { return }
        Task { @MainActor in sink(msg) }
    }

    // MARK: setup

    /// Adopt an env, binding it to `scene` for scene-ordered transforms.
    ///
    /// `authored` seeds instances with no simulated body (static scenery) and
    /// must be in scene order.
    func adopt(env newEnv: RobotEnv, scene: OpaquePointer?, authored: [float4x4]) {
        queue.async { [self] in
            stop_()
            env = newEnv
            policy = nil
            authoredTransforms = authored
            episodeReturn = 0
            if let scene { _ = newEnv.bind(scene: scene) }
        }
    }

    func release() {
        queue.async { [self] in
            stop_()
            env = nil
            policy = nil
            authoredTransforms = []
        }
    }

    func setPolicy(_ p: TrainedPolicy?) {
        queue.async { [self] in policy = p }
    }

    func setReward(_ r: RewardSpec) {
        queue.async { [self] in rewardSpec = r }
    }

    func setAutoReset(_ on: Bool) {
        queue.async { [self] in autoResetOnDone = on }
    }

    // MARK: transport

    func run() {
        queue.async { [self] in
            guard let env, timer == nil else { return }
            lastSampleTime = .now()
            stepsSinceSample = 0
            let period = env.controlDt
            let t = DispatchSource.makeTimerSource(queue: queue)
            // A leeway of a tenth of the period lets the OS coalesce wakeups
            // without meaningfully changing the pacing.
            t.schedule(deadline: .now(), repeating: period, leeway: .milliseconds(2))
            t.setEventHandler { [weak self] in self?.tick() }
            timer = t
            t.resume()
        }
    }

    func pause() {
        queue.async { [self] in stop_() }
    }

    /// Advance exactly one control step, whether or not the timer is running.
    func stepOnce() {
        queue.async { [self] in tick() }
    }

    func reset(seed newSeed: UInt64? = nil) {
        queue.async { [self] in
            guard let env else { return }
            if let newSeed { seed = newSeed }
            episodeReturn = 0
            do {
                let s = try env.reset(seed: seed)
                publish(s)
            } catch {
                report((error as? SimError)?.message ?? "\(error)")
            }
        }
    }

    /// Shove the base — a disturbance test the user can trigger by hand.
    func shove(linear: SIMD3<Double>) {
        queue.async { [self] in _ = env?.nudgeBase(linear: linear) }
    }

    private func stop_() {
        timer?.cancel()
        timer = nil
    }

    // MARK: stepping

    /// Per-phase timing, enabled with VCAD_SIM_PROFILE=1. Off by default: the
    /// question "why is the simulation not real time" has several plausible
    /// answers (physics, marshalling, the publish hop, the renderer) and
    /// guessing between them wastes more time than measuring.
    private static let profiling = ProcessInfo.processInfo.environment["VCAD_SIM_PROFILE"] == "1"
    private var profStep = 0.0, profReward = 0.0, profPublish = 0.0, profTicks = 0
    private var profLast = DispatchTime.now()

    private func tick() {
        guard let env else { return }
        let t0 = DispatchTime.now()
        do {
            let result = try policy.map { try env.step(policy: $0) }
                ?? env.step(positionTargets: [Double](repeating: 0, count: env.actionDim))
            let t1 = DispatchTime.now()
            // Reward is a client-side quantity by design; accumulate the same
            // formula training uses so the number on screen is comparable to a
            // policy's held-out score.
            episodeReturn += env.reward(rewardSpec)
            let t2 = DispatchTime.now()
            publish(result)
            if Self.profiling {
                let ms = { (a: DispatchTime, b: DispatchTime) in
                    Double(b.uptimeNanoseconds &- a.uptimeNanoseconds) / 1e6
                }
                profStep += ms(t0, t1)
                profReward += ms(t1, t2)
                profPublish += ms(t2, DispatchTime.now())
                profTicks += 1
                let since = ms(profLast, DispatchTime.now())
                if since > 1000 {
                    let n = Double(profTicks)
                    FileHandle.standardError.write(Data(
                        String(format: "[SIM_PROFILE] %.0f ticks/s  step %.2f ms  reward %.2f ms  publish %.2f ms  (budget %.1f ms)\n",
                               n / (since / 1000), profStep/n, profReward/n, profPublish/n,
                               env.controlDt * 1000).utf8))
                    profStep = 0; profReward = 0; profPublish = 0; profTicks = 0
                    profLast = DispatchTime.now()
                }
            }
            if result.done && autoResetOnDone {
                episodeReturn = 0
                seed &+= 1
                _ = try? env.reset(seed: seed)
            }
        } catch {
            stop_()
            report((error as? SimError)?.message ?? "\(error)")
        }
    }

    /// Snapshot transforms and hand everything to the main actor.
    private func publish(_ step: SimStep) {
        guard let env else { return }
        let transforms = env.sceneTransforms(fallback: authoredTransforms)

        // Real-time factor, sampled rather than computed per step so the
        // number is readable instead of flickering.
        stepsSinceSample += 1
        var rtf = Double.nan
        let now = DispatchTime.now()
        let elapsed = Double(now.uptimeNanoseconds &- lastSampleTime.uptimeNanoseconds) / 1e9
        if elapsed >= 0.25 {
            rtf = (Double(stepsSinceSample) * env.controlDt) / elapsed
            stepsSinceSample = 0
            lastSampleTime = now
        }

        // Only Sendable values cross: the closure, the transform array, the
        // step snapshot, and two doubles. `self` deliberately does not — it
        // owns a raw pointer that must never leave this queue.
        let ret = episodeReturn
        guard let sink = onFrame else { return }
        Task { @MainActor in sink(transforms, step, ret, rtf) }
    }
}

/// The observable simulation model the UI binds to.
@MainActor
@Observable
final class SimController {
    /// Sim mode is off until the user turns it on: a physics engine that
    /// starts itself would make every assembly document start falling over.
    private(set) var isAvailable = false
    private(set) var isRunning = false
    private(set) var isReady = false

    /// Latest step, for the readouts.
    private(set) var latest: SimStep = .empty
    /// Accumulated reward this episode, under `reward`.
    private(set) var episodeReturn: Double = 0
    /// Real-time factor: 1.0 means the sim is keeping up with the wall clock.
    /// `< 1` means the machine cannot step this model in real time.
    private(set) var realTimeFactor: Double = .nan
    private(set) var errorMessage: String?

    /// What is driving the robot.
    private(set) var driver: SimDriver = .restPose
    /// Set when the loaded policy no longer matches the document it trained on.
    private(set) var policyStaleWarning: String?
    /// Provenance of the loaded policy, when it came from a bundle.
    private(set) var policyBundle: PolicyBundle?

    /// Set once the user edits the spec, so `prepare` stops overwriting their
    /// choices with fresh guesses on the next rebuild.
    var configuredByUser = false
    var spec = SimSpec()
    var reward = RewardSpec()
    var trainSpec = TrainSpec()
    /// Auto-reset when an episode ends, so the viewport keeps showing motion.
    var autoReset = true { didSet { engine.setAutoReset(autoReset) } }

    /// Robot facts, for the inspector.
    private(set) var actuatedJointIDs: [String] = []
    private(set) var actionDim = 0
    private(set) var obsDim = 0
    private(set) var controlHz: Double = 0

    // Training
    private(set) var training: TrainProgress?
    private(set) var trainingError: String?
    /// Reward at each completed iteration — the curve.
    private(set) var trainingCurve: [Double] = []

    private let engine = SimEngine()
    private var trainer: Trainer?
    private var trainPollTimer: Timer?
    private var documentJSON: Data?

    /// Applied to the renderer's instance transforms. Nil when the sim isn't
    /// driving the scene.
    private(set) var transforms: [float4x4]?
    /// Bumped on every published frame so the view layer knows to re-apply.
    private(set) var frameToken = 0

    /// Set by `EditorModel` to route simulated poses into the same channel
    /// kinematic playback writes. A direct callback rather than observation:
    /// this fires at the control rate, and it must reach the renderer on the
    /// frame it was produced for, not whenever SwiftUI next recomputes.
    var onTransforms: (@MainActor ([float4x4]) -> Void)?

    init() {
        engine.setSinks(
            frame: { [weak self] transforms, step, ret, rtf in
                guard let self else { return }
                if let transforms {
                    self.transforms = transforms
                    self.onTransforms?(transforms)
                }
                self.latest = step
                self.episodeReturn = ret
                if rtf.isFinite { self.realTimeFactor = rtf }
                self.frameToken &+= 1
            },
            error: { [weak self] msg in
                self?.errorMessage = msg
                self?.isRunning = false
            })
    }

    // MARK: lifecycle

    /// Prepare a simulation for `documentJSON`, binding it to the resident
    /// scene so transforms come back in the renderer's own index order.
    ///
    /// `authored` are the scene's authored transforms, used for instances with
    /// no physics body (static scenery) so they don't collapse to the origin.
    func prepare(documentJSON doc: Data, scene: OpaquePointer?, authored: [float4x4],
                 baseDirectory: String? = nil) {
        teardown()
        self.documentJSON = doc
        errorMessage = nil
        do {
            // Derive a workable spec from the document rather than making the
            // user author one before anything moves. Everything guessed shows
            // up in the inspector, so a wrong guess is visible and correctable
            // instead of mysterious.
            var s = spec
            if !configuredByUser,
               let dict = try? JSONSerialization.jsonObject(with: doc) as? [String: Any] {
                s = SimSpec.autoConfigured(for: dict)
            }
            // Gains need the joint list, which needs an env — so build a probe
            // first. Without explicit gains a humanoid sinks through its knees.
            // A committed robot document references its vendored meshes
            // relatively; without its own directory those resolve against the
            // process working directory and every collider silently degrades to
            // a placeholder box.
            s.base_dir = baseDirectory
            if s.gains.isEmpty {
                let probe = try RobotEnv(documentJSON: doc, spec: s)
                s.gains = SimSpec.humanoidGains(for: probe.actuatedJointIDs)
            }
            let env = try RobotEnv(documentJSON: doc, spec: s)
            spec = s
            reward.nominal_height_m = s.nominal_height_m
            actuatedJointIDs = env.actuatedJointIDs
            actionDim = env.actionDim
            obsDim = env.obsDim
            controlHz = env.controlDt > 0 ? 1.0 / env.controlDt : 0
            engine.adopt(env: env, scene: scene, authored: authored)
            engine.setReward(reward)
            engine.setAutoReset(autoReset)
            engine.reset(seed: 1)
            isAvailable = true
            isReady = true
        } catch {
            // A failure here is almost always actionable — no floating base, an
            // unknown end effector, gains for a joint that doesn't exist — and
            // the ABI phrases it that way, so show it verbatim.
            errorMessage = (error as? SimError)?.message ?? "\(error)"
            isAvailable = false
            isReady = false
        }
    }

    /// Record why a simulation could not be started, for the inspector to show.
    ///
    /// Exists so `EditorModel` has no reason to fail silently: the checks it
    /// makes before handing a document over (is this an assembly, did the JSON
    /// survive a round trip) are exactly as user-visible as the kernel's own.
    func reportUnavailable(_ message: String) {
        errorMessage = message
        isAvailable = false
        isReady = false
    }

    /// Sim is not applicable to this document (not an assembly, or the user
    /// switched away).
    func teardown() {
        stopTraining()
        engine.release()
        isRunning = false
        isReady = false
        isAvailable = false
        transforms = nil
        latest = .empty
        episodeReturn = 0
        realTimeFactor = .nan
        driver = .restPose
        policyStaleWarning = nil
        policyBundle = nil
        actuatedJointIDs = []
        actionDim = 0
        obsDim = 0
    }

    // MARK: transport

    func toggleRun() { isRunning ? pause() : run() }

    func run() {
        guard isReady else { return }
        errorMessage = nil
        isRunning = true
        engine.run()
    }

    func pause() {
        isRunning = false
        engine.pause()
    }

    func stepOnce() {
        guard isReady else { return }
        pause()
        engine.stepOnce()
    }

    func reset() {
        guard isReady else { return }
        errorMessage = nil
        engine.reset(seed: 1)
    }

    /// Shove the robot sideways — the disturbance test.
    func shove(_ direction: SIMD3<Double> = SIMD3(1, 0, 0), speed: Double = 0.6) {
        guard isReady else { return }
        engine.shove(linear: direction * speed)
    }

    // MARK: policy

    /// Load a `.vcadpolicy` bundle (or a bare policy JSON) and drive with it.
    func loadPolicy(from url: URL) {
        guard let doc = documentJSON else { return }
        do {
            let data = try Data(contentsOf: url)
            let policy: TrainedPolicy
            if let p = try? TrainedPolicy(bundle: data, document: doc) {
                policy = p
            } else {
                policy = try TrainedPolicy(json: data)
            }
            // Verify against a throwaway env built from the same spec: better a
            // clear refusal now than a robot that twitches.
            let probe = try RobotEnv(documentJSON: doc, spec: spec)
            try probe.check(policy: policy)

            policyStaleWarning = policy.staleWarning
            policyBundle = policy.bundle
            driver = .policy(name: url.deletingPathExtension().lastPathComponent)
            engine.setPolicy(policy)
            errorMessage = nil
        } catch {
            errorMessage = (error as? SimError)?.message ?? "\(error)"
        }
    }

    /// Drop back to the hold-rest-pose baseline.
    func useRestPose() {
        engine.setPolicy(nil)
        driver = .restPose
        policyStaleWarning = nil
        policyBundle = nil
    }

    // MARK: training

    var isTraining: Bool { training?.running == true }

    func startTraining() {
        guard let doc = documentJSON, trainer == nil else { return }
        trainingError = nil
        trainingCurve = []
        // Keep the reward's target height and the env's agreed, or the policy
        // is told it is upright at one height and rewarded for another. The ABI
        // refuses the mismatch; matching it here means the user never sees that
        // refusal for a reason they didn't cause.
        reward.nominal_height_m = spec.nominal_height_m
        do {
            let t = try Trainer(documentJSON: doc, sim: spec, train: trainSpec, reward: reward)
            trainer = t
            training = t.poll()
            trainPollTimer = Timer.scheduledTimer(withTimeInterval: 0.25, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.pollTraining() }
            }
        } catch {
            trainingError = (error as? SimError)?.message ?? "\(error)"
        }
    }

    func stopTraining() {
        trainer?.stop()
        trainPollTimer?.invalidate()
        trainPollTimer = nil
        // Dropping the handle joins the worker (vcad_train_free blocks); that
        // is the point — a detached worker would outlive the state it reads.
        trainer = nil
        if training?.running == true { training?.running = false }
    }

    private func pollTraining() {
        guard let t = trainer else { return }
        let p = t.poll()
        if p.iteration > UInt32(trainingCurve.count) {
            // One point per completed iteration; `bestHeldOut` is the honest
            // curve, `meanReward` the noisy one. Plot the honest one.
            trainingCurve.append(p.bestHeldOut.isFinite ? p.bestHeldOut : p.meanReward)
        }
        training = p
        if p.finished {
            trainPollTimer?.invalidate()
            trainPollTimer = nil
            if p.failed { trainingError = t.errorMessage ?? "training failed" }
        }
    }

    /// Adopt the best-so-far trained policy as the live driver — "watch what it
    /// has learned" without stopping the run.
    func adoptTrainedPolicy() {
        guard let t = trainer, let data = t.bestPolicyJSON(), let doc = documentJSON else { return }
        do {
            let policy = try TrainedPolicy(bundle: data, document: doc)
            let probe = try RobotEnv(documentJSON: doc, spec: spec)
            try probe.check(policy: policy)
            policyStaleWarning = policy.staleWarning
            policyBundle = policy.bundle
            driver = .policy(name: "training best")
            engine.setPolicy(policy)
        } catch {
            trainingError = (error as? SimError)?.message ?? "\(error)"
        }
    }

    /// Write the best-so-far bundle to disk as a `.vcadpolicy`.
    @discardableResult
    func saveTrainedPolicy(to url: URL) -> Bool {
        guard let t = trainer, let data = t.bestPolicyJSON() else { return false }
        do {
            try data.write(to: url)
            return true
        } catch {
            trainingError = "\(error)"
            return false
        }
    }
}
