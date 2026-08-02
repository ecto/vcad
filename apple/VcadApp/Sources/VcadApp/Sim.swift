import Foundation
import simd
import CVcadFFI

// Physics simulation and policy inference, over the `vcad_gym_*` / `vcad_train_*`
// C ABI (crates/vcad-ffi/src/gym.rs, train.rs).
//
// Layering, deliberately:
//
//   SimSpec / RewardSpec / TrainSpec   — Codable mirrors of the Rust structs.
//   RobotEnv / TrainedPolicy / Trainer — thin RAII handles. No policy of their own.
//   SimEngine                          — owns the env on a private serial queue.
//   SimController                      — @MainActor @Observable, what the UI binds to.
//
// The split exists because physics must not run on the main thread. One control
// step of a 23-DOF humanoid is 20 physics substeps; at 50 Hz that is a
// millisecond-scale chunk of work landing on every frame, and doing it inline
// turns a dropped frame into a *slower simulation*, which then looks like a
// physics bug. So the handle lives on `SimEngine.queue` and is never touched
// from anywhere else, and the UI sees only value types published back.

// MARK: - Errors

/// A failure reported across the FFI boundary, carrying the kernel's own
/// explanation rather than a generic "simulation failed".
struct SimError: LocalizedError {
    let message: String
    var errorDescription: String? { message }

    /// Read the thread-local last error the ABI just recorded.
    ///
    /// Must be called on the same thread as the failing call — the slot is
    /// thread-local precisely so two threads failing at once cannot overwrite
    /// each other's diagnosis.
    static func fromFFI(_ fallback: String) -> SimError {
        var len = 0
        guard let p = vcad_last_error(&len), len > 0 else {
            return SimError(message: fallback)
        }
        let bytes = UnsafeBufferPointer(start: p, count: len)
        return SimError(message: String(decoding: bytes, as: UTF8.self))
    }

    /// The last error, or nil when the ABI recorded none. Used where a call
    /// *succeeds* but wants to report something (a stale policy, say).
    static func pending() -> String? {
        var len = 0
        guard let p = vcad_last_error(&len), len > 0 else { return nil }
        return String(decoding: UnsafeBufferPointer(start: p, count: len), as: UTF8.self)
    }
}

/// Column-major 16-double slice -> `float4x4`, the layout every vcad transform
/// entry point writes.
///
/// A free function rather than `EditorModel.mat4` because this runs on the
/// simulation queue: the editor's copy is `@MainActor`-isolated, and reaching
/// for it from a background queue is exactly the data race Swift 6 refuses.
func vcadMat4(_ d: [Double], at o: Int = 0) -> float4x4 {
    func col(_ c: Int) -> SIMD4<Float> {
        SIMD4<Float>(Float(d[o + c * 4]), Float(d[o + c * 4 + 1]),
                     Float(d[o + c * 4 + 2]), Float(d[o + c * 4 + 3]))
    }
    return float4x4(columns: (col(0), col(1), col(2), col(3)))
}

/// Borrow a `(ptr, len)` UTF-8 pair as a Swift `String`.
private func ffiString(_ p: UnsafePointer<UInt8>?, _ len: Int) -> String? {
    guard let p, len > 0 else { return nil }
    return String(decoding: UnsafeBufferPointer(start: p, count: len), as: UTF8.self)
}

// MARK: - Specs (mirrors of the Rust serde structs)

/// Domain randomization ranges. All optional; an unset channel is off.
struct Randomization: Codable, Equatable {
    struct Range: Codable, Equatable {
        var min: Double
        var max: Double
        /// A symmetric ±`pct` multiplicative range (0.1 → 0.9…1.1).
        static func plusMinus(_ pct: Double) -> Range { Range(min: 1 - pct, max: 1 + pct) }
    }
    var mass_scale: Range?
    var friction_scale: Range?
    var pd_gain_scale: Range?
    /// Actuator latency in physics substeps, `[min, max]`.
    var action_latency_steps: [UInt32]?
    var joint_pos_perturb: Double?
    var joint_vel_perturb: Double?

    /// Booster's own K1-shaped sim2real settings, the ones the bundled
    /// policies trained under.
    static let sim2real = Randomization(
        mass_scale: .plusMinus(0.1),
        pd_gain_scale: .plusMinus(0.2),
        action_latency_steps: [2, 8],
        joint_pos_perturb: 2.0,
        joint_vel_perturb: 5.0
    )
}

/// When an episode ends.
struct Termination: Codable, Equatable {
    var base_height_below: Double?
    var base_tilt_above_deg: Double?
    var terminate_on_joint_limit: Bool = false
}

/// Gaussian sensor noise applied to `step`/`reset` observations only — the
/// true state stays clean for reward and termination.
struct ObservationNoise: Codable, Equatable {
    var joint_pos_std: Double = 0
    var joint_vel_std: Double = 0
    var base_pos_std: Double = 0
    var base_rot_std: Double = 0
    var base_vel_std: Double = 0
    var contact_force_std: Double = 0
}

struct EnvConfig: Codable, Equatable {
    var randomization: Randomization?
    var observation_noise: ObservationNoise?
    var termination: Termination?
    var base_instance_id: String?
}

/// Everything needed to build an environment. Mirrors `vcad_ffi::gym::GymSpec`.
///
/// Encoded with `deny_unknown_fields` on the Rust side, so a field renamed
/// here without renaming there fails loudly at `create` instead of silently
/// falling back to a default.
struct SimSpec: Codable, Equatable {
    var end_effector_ids: [String] = []
    /// Physics timestep. **The field to get right.** A policy trained at one
    /// `dt` and replayed at another sees a different plant — stiff humanoid
    /// gains sit near their explicit-integration stability limit at 1 kHz and
    /// diverge outright at 200 Hz.
    var dt: Double = 1.0 / 1000.0
    var substeps: UInt32 = 20
    var max_steps: UInt32 = 400
    /// `joint_id -> [kp, kd]`.
    var gains: [String: [Double]] = [:]
    var config: EnvConfig = EnvConfig()
    var nominal_height_m: Double = 0
    var spawn_z_mm: Double?
    /// Directory to resolve relative `MeshImport` paths against — the
    /// document's own location. A committed robot document references its
    /// vendored meshes relatively.
    var base_dir: String?
    var require_floating_base: Bool = true

    /// Control period in seconds — the rate `step` should be called at for
    /// real-time playback.
    var controlDt: Double { dt * Double(max(substeps, 1)) }

    func encoded() throws -> Data { try JSONEncoder().encode(self) }

    /// Derive a workable spec by inspecting the document.
    ///
    /// The alternative is making the user hand-write a spec before anything
    /// moves, which means the first experience of the feature is an error
    /// message about a field they have never heard of. Everything guessed here
    /// is visible in the inspector and overridable; the guesses are:
    ///
    /// - **Floating base.** A `Free` joint means the robot can fall, so
    ///   termination and a nominal height make sense. Without one it is bolted
    ///   to the world, so `require_floating_base` is relaxed and the
    ///   height/tilt terminations are left off — they would be constants.
    /// - **Nominal height** comes from the free joint's spawn anchor, which is
    ///   where the document says the robot stands.
    /// - **End effectors** are instances whose names look like feet. Only feet
    ///   have a contact channel worth observing on a balance task, and a wrong
    ///   guess here is visible (the contact readout stays blank) rather than
    ///   silent.
    static func autoConfigured(for document: [String: Any]) -> SimSpec {
        var spec = SimSpec()
        let instances = (document["instances"] as? [[String: Any]] ?? [])
            .compactMap { $0["id"] as? String }
        let joints = document["joints"] as? [[String: Any]] ?? []

        let freeJoint = joints.first {
            (($0["kind"] as? [String: Any])?["type"] as? String) == "Free"
        }

        if let free = freeJoint {
            spec.require_floating_base = true
            // The free joint's child is the trunk/base link.
            spec.config.base_instance_id = free["childInstanceId"] as? String
            let spawnMM = (free["parentAnchor"] as? [String: Any])?["z"] as? Double ?? 0
            spec.nominal_height_m = spawnMM / 1000.0
            // Tilt is the signal that generalizes: a robot tipped past 45° has
            // fallen over, whether it stands two metres tall or sits on the
            // floor. A height floor only means something when the base starts
            // genuinely elevated — derive one for a robot resting on the ground
            // and it either never fires or fires on contact penetration, and in
            // both cases it is measuring nothing.
            let elevated = spec.nominal_height_m > 0.2
            spec.config.termination = Termination(
                base_height_below: elevated ? spec.nominal_height_m * 0.55 : nil,
                base_tilt_above_deg: 45.0)
        } else {
            // Bolted to the world: it cannot fall, so height and tilt
            // predicates would be constants that never fire.
            spec.require_floating_base = false
            // An *empty* termination config, deliberately not `nil`. Leaving it
            // nil looks like "no termination" but selects the kernel's legacy
            // behaviour instead: terminate as soon as any end effector is below
            // ground — which for a robot standing on the ground is true on step
            // one, every episode. With auto-reset on, that presents as a robot
            // that visibly does nothing while the step counter flickers 0-1-0.
            spec.config.termination = Termination()
        }

        spec.end_effector_ids = instances.filter { id in
            let l = id.lowercased()
            return l.contains("foot") || l.contains("ankle_roll") || l.contains("toe")
        }
        return spec
    }

    /// booster_gym-style gains for a humanoid: stiff hips/knees, soft ankles,
    /// soft upper body. These need the 1 kHz tick; the kernel's inertia-scaled
    /// defaults are stable at any tick but far too soft to hold a humanoid up,
    /// so the robot sinks through its knees and the policy learns to fight its
    /// own springs.
    static func humanoidGains(for jointIDs: [String]) -> [String: [Double]] {
        var out: [String: [Double]] = [:]
        for id in jointIDs {
            let lower = id.lowercased()
            if lower.contains("hip") || lower.contains("knee") {
                out[id] = [200, 5]
            } else if lower.contains("ankle") {
                out[id] = [50, 1]
            } else {
                out[id] = [40, 1]
            }
        }
        return out
    }
}

/// The standing-balance reward, as data. Mirrors `vcad_ffi::train::RewardSpec`;
/// defaults are the measured K1 weights.
struct RewardSpec: Codable, Equatable {
    var alive: Double = 1.0
    var nominal_height_m: Double = 0.5498
    var height: Double = 8.0
    var tilt: Double = 1.5
    var drift: Double = 0.3
    var spin: Double = 0.05
    var effort: Double = 0.1
    var effort_scale_deg: Double = 30.0

    func encoded() throws -> Data { try JSONEncoder().encode(self) }
}

/// ARS hyperparameters. Mirrors `vcad_sim::rl::ArsConfig`.
struct ArsConfig: Codable, Equatable {
    var n_directions: Int = 12
    var top_k: Int = 6
    /// Step size α. Small on purpose: ARS's update is scale-free, so it takes
    /// the same-size step when the policy is optimal as when it is useless. At
    /// α = 0.03 a run walks back out of every solution it finds.
    var step_size: Double = 0.005
    var step_size_final: Double?
    var noise_std: Double = 0.05
    var iterations: Int = 150
    var rollouts_per_eval: Int = 3
    var seed: UInt64 = 7
}

/// Mirrors `vcad_ffi::train::TrainSpec`.
struct TrainSpec: Codable, Equatable {
    var ars = ArsConfig()
    /// `"mlp"` or `"linear"`. MLP by a wide margin on balance tasks — balance
    /// switches contact mode and one gain matrix has to serve every mode.
    var policy: String = "mlp"
    var hidden: Int = 64
    var action_scale_deg: Double = 8.0
    var init_seed: UInt64 = 0
    var curriculum_warmup: Double = 0.4
    var held_out_every: Int = 5
    var held_out_seeds: Int = 10

    func encoded() throws -> Data { try JSONEncoder().encode(self) }
}

/// A trained policy plus the provenance that says whether it still applies.
/// Mirrors `vcad_ffi::train::PolicyBundle` — the `.vcadpolicy` payload.
struct PolicyBundle: Codable {
    var policy: AnyCodable
    var kept: String
    var held_out_reward: Double
    var held_out_full_episodes: Int
    var held_out_seeds: Int
    var env: SimSpec
    var reward: RewardSpec
    var ars: ArsConfig
    var document_hash: String
    var version: Int
}

/// Minimal `Codable` box for the opaque policy-weights object, which Swift
/// never needs to inspect — inference happens in Rust so the forward pass
/// cannot drift from training.
struct AnyCodable: Codable {
    let json: Data
    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        // Round-trip through JSONSerialization: the payload is a weights blob,
        // and re-encoding it verbatim is all that is ever asked of it.
        let value = try c.decode(JSONValue.self)
        json = try JSONEncoder().encode(value)
    }
    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        try c.encode(JSONDecoder().decode(JSONValue.self, from: json))
    }
}

/// A structural JSON value, used only to carry the weights blob intact.
indirect enum JSONValue: Codable {
    case null, bool(Bool), number(Double), string(String)
    case array([JSONValue]), object([String: JSONValue])

    init(from decoder: Decoder) throws {
        let c = try decoder.singleValueContainer()
        if c.decodeNil() { self = .null }
        else if let v = try? c.decode(Bool.self) { self = .bool(v) }
        else if let v = try? c.decode(Double.self) { self = .number(v) }
        else if let v = try? c.decode(String.self) { self = .string(v) }
        else if let v = try? c.decode([JSONValue].self) { self = .array(v) }
        else if let v = try? c.decode([String: JSONValue].self) { self = .object(v) }
        else {
            throw DecodingError.dataCorruptedError(in: c, debugDescription: "unsupported JSON")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let v): try c.encode(v)
        case .number(let v): try c.encode(v)
        case .string(let v): try c.encode(v)
        case .array(let v): try c.encode(v)
        case .object(let v): try c.encode(v)
        }
    }
}

// MARK: - Observation snapshot

/// Ground contact under one end effector — what a foot force plate reads.
struct FootContact: Equatable, Sendable {
    var inContact: Bool
    var normalForce: Double
    var centerOfPressure: SIMD3<Double>
}

/// A value-type copy of one step's result, safe to hand to the main actor.
///
/// A copy rather than the borrowed FFI view because the view's pointers are
/// invalidated by the next step — and the next step happens on the sim queue
/// while the UI is still drawing the last one.
struct SimStep: Equatable, Sendable {
    var step: UInt32
    var reward: Double
    var done: Bool
    var terminated: Bool
    var truncated: Bool
    /// Noise-free base height (m). Ground truth: deliberately not equal to the
    /// observed base pose when sensor noise is configured.
    var baseHeightM: Double
    /// Noise-free tilt from upright (degrees).
    var baseTiltDeg: Double
    var hasBase: Bool
    var terminationReason: String?
    var actionLatencySubsteps: UInt32
    var jointPositions: [Double]
    var jointVelocities: [Double]
    var footContacts: [FootContact]

    static let empty = SimStep(
        step: 0, reward: 0, done: false, terminated: false, truncated: false,
        baseHeightM: 0, baseTiltDeg: 0, hasBase: false, terminationReason: nil,
        actionLatencySubsteps: 0, jointPositions: [], jointVelocities: [], footContacts: []
    )
}

// MARK: - Handles

/// An owned physics environment.
///
/// Not `Sendable`: it wraps a raw pointer whose interior is mutated by every
/// call. `SimEngine` confines it to one serial queue; nothing else may hold it.
final class RobotEnv {
    fileprivate let handle: OpaquePointer
    let spec: SimSpec

    /// Actuated joint ids, in action-vector order.
    let actuatedJointIDs: [String]
    /// Simulated body ids, in `bodyTransforms` order.
    let bodyIDs: [String]
    let actionDim: Int
    /// Policy feature count (not the raw observation size).
    let obsDim: Int

    init(documentJSON: Data, spec: SimSpec) throws {
        let specData = try spec.encoded()
        let h: OpaquePointer? = documentJSON.withUnsafeBytes { doc in
            specData.withUnsafeBytes { sp in
                vcad_gym_create(
                    doc.bindMemory(to: UInt8.self).baseAddress, documentJSON.count,
                    sp.bindMemory(to: UInt8.self).baseAddress, specData.count)
            }
        }
        guard let h else { throw SimError.fromFFI("could not create simulation") }
        handle = h
        self.spec = spec
        actionDim = vcad_gym_action_dim(h)
        obsDim = vcad_gym_obs_dim(h)
        actuatedJointIDs = (0..<vcad_gym_actuated_joint_count(h)).compactMap { i in
            var len = 0
            return ffiString(vcad_gym_actuated_joint_id(h, i, &len), len)
        }
        bodyIDs = (0..<vcad_gym_body_count(h)).compactMap { i in
            var len = 0
            return ffiString(vcad_gym_body_id(h, i, &len), len)
        }
    }

    deinit { vcad_gym_free(handle) }

    /// Control period in seconds, straight from the kernel rather than
    /// recomputed here — one source of truth for the rate everything runs at.
    var controlDt: Double { vcad_gym_control_dt(handle) }
    var maxSteps: UInt32 { vcad_gym_max_steps(handle) }

    @discardableResult
    func reset(seed: UInt64) throws -> SimStep {
        guard vcad_gym_reset(handle, seed) == 1 else {
            throw SimError.fromFFI("reset failed")
        }
        return snapshot()
    }

    /// Step with explicit position targets (degrees).
    @discardableResult
    func step(positionTargets: [Double]) throws -> SimStep {
        guard positionTargets.count == actionDim else {
            throw SimError(message:
                "action has \(positionTargets.count) values, env expects \(actionDim)")
        }
        let ok = positionTargets.withUnsafeBufferPointer {
            vcad_gym_step(handle, $0.baseAddress, $0.count, 1)
        }
        guard ok == 1 else { throw SimError.fromFFI("step failed") }
        return snapshot()
    }

    /// Step by evaluating `policy`.
    ///
    /// The whole observation → features → action chain stays inside Rust, so
    /// it cannot drift from what training did. Rebuilding any link of it here
    /// is how a policy silently degrades.
    @discardableResult
    func step(policy: TrainedPolicy) throws -> SimStep {
        guard vcad_gym_policy_step(handle, policy.handle) == 1 else {
            throw SimError.fromFFI("policy step failed")
        }
        return snapshot()
    }

    /// Check a policy against this env before running it. Worth doing once at
    /// load: a mismatch otherwise produces numbers rather than an error.
    func check(policy: TrainedPolicy) throws {
        guard vcad_policy_check(handle, policy.handle) == 1 else {
            throw SimError.fromFFI("policy is not compatible with this robot")
        }
    }

    /// A zero (hold-rest-pose) policy matched to this env — the baseline every
    /// trained policy must beat, and a safe default before one is loaded.
    func zeroPolicy(actionScaleDeg: Double = 8.0) throws -> TrainedPolicy {
        guard let p = vcad_policy_zeros(handle, actionScaleDeg) else {
            throw SimError.fromFFI("could not build the baseline policy")
        }
        return TrainedPolicy(adopting: p)
    }

    /// Shove the floating base — angular (rad/s) then body-frame linear (m/s).
    @discardableResult
    func nudgeBase(angular: SIMD3<Double> = .zero, linear: SIMD3<Double>) -> Bool {
        vcad_gym_nudge_base(handle, angular.x, angular.y, angular.z,
                            linear.x, linear.y, linear.z) == 1
    }

    /// Evaluate a reward against the most recent step, using the same formula
    /// training uses.
    func reward(_ spec: RewardSpec) -> Double {
        guard let data = try? spec.encoded() else { return .nan }
        return data.withUnsafeBytes {
            vcad_gym_reward(handle, $0.bindMemory(to: UInt8.self).baseAddress, data.count)
        }
    }

    /// Bind to a scene's instance ordering so transforms come back in the
    /// index space the renderer already uses. Returns the match count.
    @discardableResult
    func bind(scene: OpaquePointer) -> Int {
        vcad_gym_bind_scene(handle, scene)
    }

    var sceneBindingCount: Int { vcad_gym_scene_binding_len(handle) }

    /// Simulated transforms in **scene instance order**, millimetres,
    /// column-major — drop-in for the kinematic FK path.
    ///
    /// `fallback` seeds instances with no simulated body (static scenery), and
    /// must already be in scene order. Returns nil when nothing is bound.
    func sceneTransforms(fallback: [float4x4]) -> [float4x4]? {
        let n = sceneBindingCount
        guard n > 0 else { return nil }
        var buf = [Double](repeating: 0, count: n * 16)
        // Pre-fill with the authored transforms; the ABI leaves unmatched
        // instances untouched, so anything without a body keeps its pose
        // instead of collapsing to the origin.
        for i in 0..<min(n, fallback.count) {
            let m = fallback[i]
            for c in 0..<4 {
                let col = m[c]
                buf[i * 16 + c * 4 + 0] = Double(col.x)
                buf[i * 16 + c * 4 + 1] = Double(col.y)
                buf[i * 16 + c * 4 + 2] = Double(col.z)
                buf[i * 16 + c * 4 + 3] = Double(col.w)
            }
        }
        let written = buf.withUnsafeMutableBufferPointer {
            vcad_gym_scene_transforms(handle, $0.baseAddress, $0.count)
        }
        guard written == n else { return nil }
        return (0..<n).map { vcadMat4(buf, at: $0 * 16) }
    }

    /// Copy the current borrowed view into a value type.
    private func snapshot() -> SimStep {
        let v = vcad_gym_step_view(handle)
        func doubles(_ p: UnsafePointer<Double>?, _ n: Int) -> [Double] {
            guard let p, n > 0 else { return [] }
            return Array(UnsafeBufferPointer(start: p, count: n))
        }
        let contactsFlat = doubles(v.end_effector_contacts, v.end_effector_contacts_len)
        var contacts: [FootContact] = []
        contacts.reserveCapacity(contactsFlat.count / 5)
        for i in stride(from: 0, to: contactsFlat.count - 4, by: 5) {
            contacts.append(FootContact(
                inContact: contactsFlat[i] != 0,
                normalForce: contactsFlat[i + 1],
                centerOfPressure: SIMD3(contactsFlat[i + 2], contactsFlat[i + 3], contactsFlat[i + 4])))
        }
        return SimStep(
            step: v.step,
            reward: v.reward,
            done: v.done != 0,
            terminated: v.terminated != 0,
            truncated: v.truncated != 0,
            baseHeightM: v.base_height_m,
            baseTiltDeg: v.base_tilt_deg,
            hasBase: v.has_base != 0,
            terminationReason: ffiString(v.termination_reason, v.termination_reason_len),
            actionLatencySubsteps: v.action_latency_substeps,
            jointPositions: doubles(v.joint_positions, v.joint_positions_len),
            jointVelocities: doubles(v.joint_velocities, v.joint_velocities_len),
            footContacts: contacts)
    }
}

/// An owned trained policy.
final class TrainedPolicy {
    fileprivate let handle: OpaquePointer
    /// Set when the policy loaded but its document hash no longer matches —
    /// the Stale case. It still runs; whether that is meaningful is a
    /// judgement the UI surfaces rather than a load error.
    private(set) var staleWarning: String?
    private(set) var bundle: PolicyBundle?

    fileprivate init(adopting h: OpaquePointer) { handle = h }

    /// Load a bare policy JSON (no provenance).
    init(json: Data) throws {
        guard let h = json.withUnsafeBytes({
            vcad_policy_load($0.bindMemory(to: UInt8.self).baseAddress, json.count)
        }) else { throw SimError.fromFFI("could not load policy") }
        handle = h
    }

    /// Load a `.vcadpolicy` bundle, checking it against the document it will
    /// run on.
    init(bundle data: Data, document: Data?) throws {
        let h: OpaquePointer? = data.withUnsafeBytes { b in
            let bp = b.bindMemory(to: UInt8.self).baseAddress
            if let document {
                return document.withUnsafeBytes { d in
                    vcad_policy_load_bundle(bp, data.count,
                                            d.bindMemory(to: UInt8.self).baseAddress,
                                            document.count)
                }
            }
            return vcad_policy_load_bundle(bp, data.count, nil, 0)
        }
        guard let h else { throw SimError.fromFFI("could not load policy bundle") }
        handle = h
        // A successful load may still have recorded a staleness note.
        if let msg = SimError.pending(), msg.contains("STALE") { staleWarning = msg }
        self.bundle = try? JSONDecoder().decode(PolicyBundle.self, from: data)
    }

    deinit { vcad_policy_free(handle) }

    var obsDim: Int { vcad_policy_obs_dim(handle) }
    var actDim: Int { vcad_policy_act_dim(handle) }
    var isMLP: Bool { vcad_policy_is_mlp(handle) == 1 }
    var architecture: String { isMLP ? "MLP" : "linear" }
}

/// Live training progress, copied out of the worker on every poll.
struct TrainProgress: Equatable, Sendable {
    var iteration: UInt32 = 0
    var totalIterations: UInt32 = 0
    var meanReward: Double = 0
    /// The trainer's own eval. **Not** a trustworthy measure of an iterate on
    /// a randomized env — shown for diagnosis, never for selection.
    var evalReward: Double = 0
    var evalSteps: UInt32 = 0
    /// Top-k spread. Watch it: collapsing toward zero one or two iterations
    /// before the return falls apart is the classic ARS divergence.
    var sigma: Double = 0
    var updateNorm: Double = 0
    var stepSize: Double = 0
    /// The only number a run may be judged by.
    var bestHeldOut: Double = -.infinity
    var bestHeldOutFull: UInt32 = 0
    var bestIteration: UInt32 = 0
    var running = false
    var finished = false
    var failed = false
    var cancelled = false

    var fraction: Double {
        totalIterations == 0 ? 0 : min(1, Double(iteration) / Double(totalIterations))
    }
}

/// An in-process ARS training run.
final class Trainer {
    private let handle: OpaquePointer

    init(documentJSON: Data, sim: SimSpec, train: TrainSpec, reward: RewardSpec) throws {
        let simData = try sim.encoded()
        let trainData = try train.encoded()
        let rewardData = try reward.encoded()
        let h: OpaquePointer? = documentJSON.withUnsafeBytes { d in
            simData.withUnsafeBytes { s in
                trainData.withUnsafeBytes { t in
                    rewardData.withUnsafeBytes { r in
                        vcad_train_start(
                            d.bindMemory(to: UInt8.self).baseAddress, documentJSON.count,
                            s.bindMemory(to: UInt8.self).baseAddress, simData.count,
                            t.bindMemory(to: UInt8.self).baseAddress, trainData.count,
                            r.bindMemory(to: UInt8.self).baseAddress, rewardData.count)
                    }
                }
            }
        }
        guard let h else { throw SimError.fromFFI("could not start training") }
        handle = h
    }

    /// `vcad_train_free` cancels and JOINS the worker, so this blocks briefly.
    /// That is deliberate — detaching would leave a thread stepping physics
    /// against freed state.
    deinit { vcad_train_free(handle) }

    func poll() -> TrainProgress {
        var raw = VcadTrainProgress()
        guard vcad_train_poll(handle, &raw) == 1 else { return TrainProgress() }
        return TrainProgress(
            iteration: raw.iteration,
            totalIterations: raw.total_iterations,
            meanReward: raw.mean_reward,
            evalReward: raw.eval_reward,
            evalSteps: raw.eval_steps,
            sigma: raw.sigma,
            updateNorm: raw.update_norm,
            stepSize: raw.step_size,
            bestHeldOut: raw.best_held_out,
            bestHeldOutFull: raw.best_held_out_full,
            bestIteration: raw.best_iteration,
            running: raw.running == 1,
            finished: raw.finished == 1,
            failed: raw.failed == 1,
            cancelled: raw.cancelled == 1)
    }

    func stop() { vcad_train_stop(handle) }

    var errorMessage: String? {
        var len = 0
        return ffiString(vcad_train_error(handle, &len), len)
    }

    /// The best-by-held-out policy bundle so far, or nil if none scored yet.
    ///
    /// The ABI's two-call size-then-copy protocol: the sizing call returns the
    /// required byte count directly. (It used to return 0 and report the size
    /// only in the error message, so this had to regex a number out of English
    /// prose — which would have broken silently, by failing to save a trained
    /// policy, the first time anyone reworded the message.)
    func bestPolicyJSON() -> Data? {
        let size = vcad_train_best_policy_json(handle, nil, 0)
        guard size > 0 else { return nil }
        var buf = [UInt8](repeating: 0, count: size)
        let written = buf.withUnsafeMutableBufferPointer {
            vcad_train_best_policy_json(handle, $0.baseAddress, $0.count)
        }
        guard written == size else { return nil }
        return Data(buf)
    }
}

/// The ABI version of the `libvcad_ffi.a` this binary actually linked.
///
/// Worth asserting at startup and in tests: a stale static library is the
/// classic native-app failure, and because C has no signature checking the
/// symptom is corruption rather than a link error.
func vcadFFIABIVersion() -> UInt32 { vcad_ffi_abi_version() }

/// Content hash of a document, matching the one recorded in a policy bundle.
func vcadDocumentHash(_ documentJSON: Data) -> String? {
    var buf = [UInt8](repeating: 0, count: 64)
    let n = documentJSON.withUnsafeBytes { d in
        buf.withUnsafeMutableBufferPointer {
            vcad_document_hash(d.bindMemory(to: UInt8.self).baseAddress, documentJSON.count,
                               $0.baseAddress, $0.count)
        }
    }
    guard n > 0 else { return nil }
    return String(decoding: buf[0..<n], as: UTF8.self)
}
