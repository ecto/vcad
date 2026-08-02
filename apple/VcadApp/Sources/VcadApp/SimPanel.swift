import SwiftUI
import Charts
import UniformTypeIdentifiers

// The Dojo: simulation transport, live telemetry, and the training console.
//
// Presentation rules this panel follows, because they are the difference
// between a readable instrument and a wall of numbers:
//
// - Ground-truth quantities (height, tilt) are labelled as such where they can
//   be confused with the noisy sensor values a policy sees.
// - The trainer's own eval return is shown but visually demoted: on a
//   randomized env it is not a trustworthy measure of an iterate, and a chart
//   that plots it as *the* curve teaches the user to trust it.
// - Held-out return is the headline number, because it is the only one a run
//   may be judged by.

/// Compact transport, docked beside the kinematic playback bar.
struct SimBar: View {
    @Bindable var model: EditorModel

    private var sim: SimController { model.sim }

    var body: some View {
        HStack(spacing: 10) {
            if !sim.isAvailable {
                Button {
                    model.enableSimulation()
                } label: {
                    Label("Simulate", systemImage: "atom")
                        .font(.system(size: 11, weight: .semibold))
                }
                .buttonStyle(.plain)
                .help("Build a physics simulation for this assembly")
            } else {
                Button { sim.toggleRun() } label: {
                    Image(systemName: sim.isRunning ? "pause.fill" : "play.fill")
                        .font(.system(size: 13, weight: .semibold))
                        .frame(width: 22, height: 22)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .help(sim.isRunning ? "Pause simulation" : "Run simulation")

                Button { sim.stepOnce() } label: {
                    Image(systemName: "forward.frame.fill").font(.system(size: 12))
                        .frame(width: 20, height: 22).contentShape(Rectangle())
                }
                .buttonStyle(.plain).help("Step one control tick")

                Button { sim.reset() } label: {
                    Image(systemName: "arrow.counterclockwise").font(.system(size: 12))
                        .frame(width: 20, height: 22).contentShape(Rectangle())
                }
                .buttonStyle(.plain).help("Reset the episode")

                Button { sim.shove() } label: {
                    Image(systemName: "hand.point.right.fill").font(.system(size: 12))
                        .frame(width: 20, height: 22).contentShape(Rectangle())
                }
                .buttonStyle(.plain).help("Shove the robot (disturbance test)")

                Divider().frame(height: 16)

                SimReadout(sim: sim)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 8)
        .glassCard(22)
    }
}

/// The always-visible numbers: step, height, tilt, return, real-time factor.
private struct SimReadout: View {
    let sim: SimController

    var body: some View {
        HStack(spacing: 12) {
            metric("step", "\(sim.latest.step)/\(sim.spec.max_steps)")
            if sim.latest.hasBase {
                metric("height", String(format: "%.3f m", sim.latest.baseHeightM))
                metric("tilt", String(format: "%.1f°", sim.latest.baseTiltDeg))
            }
            metric("return", String(format: "%.1f", sim.episodeReturn))
            if sim.realTimeFactor.isFinite {
                metric("rtf", String(format: "%.2f×", sim.realTimeFactor))
                    // Below ~0.9 the machine is not keeping up, which changes
                    // how the motion reads on screen; say so rather than let it
                    // look like sluggish physics.
                    .foregroundStyle(sim.realTimeFactor < 0.9 ? .orange : .secondary)
            }
        }
        .font(.system(size: 10, weight: .medium, design: .monospaced))
        .foregroundStyle(.secondary)
    }

    @ViewBuilder
    private func metric(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label.uppercased())
                .font(.system(size: 8, weight: .semibold))
                .foregroundStyle(.tertiary)
            Text(value)
        }
    }
}

/// The full inspector page: driver, telemetry, env settings, training.
struct SimInspector: View {
    @Bindable var model: EditorModel
    @State private var showingTrainer = false

    private var sim: SimController { model.sim }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            if let err = sim.errorMessage {
                // Simulation failures are almost always actionable — no
                // floating base, an unknown end effector, gains for a joint
                // that doesn't exist — and the kernel phrases them that way,
                // so show it verbatim instead of a generic banner.
                Banner(text: err, tone: .error)
            }

            if !sim.isAvailable {
                unavailable
            } else {
                driverSection
                telemetrySection
                envSection
                trainingSection
            }
        }
    }

    // MARK: sections

    private var unavailable: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("No simulation", systemImage: "atom")
                .font(.system(size: 12, weight: .semibold))
            Text(model.canSimulate
                 ? "Build a physics simulation for this assembly."
                 : "Open an assembly document with joints to simulate it.")
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            if model.canSimulate {
                Button("Simulate") { model.enableSimulation() }
                    .controlSize(.small)
            }
        }
    }

    private var driverSection: some View {
        section("Driver") {
            HStack(spacing: 8) {
                Text(sim.driver.label)
                    .font(.system(size: 11, weight: .medium))
                Spacer()
                Button("Load policy…") { loadPolicy() }
                    .controlSize(.small)
                if case .policy = sim.driver {
                    Button("Rest pose") { sim.useRestPose() }
                        .controlSize(.small)
                }
            }

            if let warning = sim.policyStaleWarning {
                // The receipt behaviour: the policy still runs, but its score
                // does not describe this model any more.
                Banner(text: warning, tone: .warning)
            }

            if let b = sim.policyBundle {
                VStack(alignment: .leading, spacing: 2) {
                    keyValue("held-out",
                             String(format: "%.2f over %d seeds (%d full episodes)",
                                    b.held_out_reward, b.held_out_seeds,
                                    b.held_out_full_episodes))
                    keyValue("trained at",
                             String(format: "%.0f Hz physics / %.0f Hz control",
                                    1 / b.env.dt, 1 / b.env.controlDt))
                    keyValue("kept", b.kept)
                    keyValue("model", String(b.document_hash.prefix(20)))
                }
            }
        }
    }

    private var telemetrySection: some View {
        section("Telemetry") {
            let s = sim.latest
            keyValue("actuated joints", "\(sim.actionDim)")
            keyValue("policy features", "\(sim.obsDim)")
            keyValue("control rate", String(format: "%.0f Hz", sim.controlHz))
            if s.hasBase {
                // Named "true" because with observation noise configured these
                // deliberately differ from what the policy sees.
                keyValue("true height", String(format: "%.4f m", s.baseHeightM))
                keyValue("true tilt", String(format: "%.2f°", s.baseTiltDeg))
            }
            if !s.footContacts.isEmpty {
                keyValue("foot contact", s.footContacts.map {
                    $0.inContact ? String(format: "%.0fN", $0.normalForce) : "—"
                }.joined(separator: "  /  "))
            }
            if s.actionLatencySubsteps > 0 {
                keyValue("actuator latency", "\(s.actionLatencySubsteps) substeps")
            }
            if let reason = s.terminationReason {
                keyValue("terminated", reason)
            }
            Toggle("Auto-reset on episode end", isOn: Binding(
                get: { sim.autoReset }, set: { sim.autoReset = $0 }))
                .font(.system(size: 11))
                .controlSize(.small)
        }
    }

    private var envSection: some View {
        section("Environment") {
            keyValue("physics", String(format: "%.0f Hz", 1 / sim.spec.dt))
            keyValue("substeps", "\(sim.spec.substeps)")
            keyValue("episode", "\(sim.spec.max_steps) steps")
            if let t = sim.spec.config.termination {
                if let h = t.base_height_below {
                    keyValue("terminate below", String(format: "%.2f m", h))
                }
                if let a = t.base_tilt_above_deg {
                    keyValue("terminate beyond", String(format: "%.0f°", a))
                }
            }
            keyValue("randomization",
                     sim.spec.config.randomization == nil ? "off" : "sim2real")
        }
    }

    private var trainingSection: some View {
        section("Training") {
            if let p = sim.training {
                trainingProgress(p)
            } else {
                Text("Search for a balance policy with ARS, in-process.")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                HStack {
                    Button("Train") { sim.startTraining() }
                        .controlSize(.small)
                    Text("\(sim.trainSpec.ars.iterations) iterations · \(sim.trainSpec.policy)")
                        .font(.system(size: 10, design: .monospaced))
                        .foregroundStyle(.tertiary)
                }
            }
            if let e = sim.trainingError {
                Banner(text: e, tone: .error)
            }
        }
    }

    @ViewBuilder
    private func trainingProgress(_ p: TrainProgress) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            ProgressView(value: p.fraction)
                .controlSize(.small)

            HStack(spacing: 14) {
                stat("iteration", "\(p.iteration)/\(p.totalIterations)")
                // The headline: the only number a run may be judged by.
                stat("held-out", p.bestHeldOut.isFinite
                     ? String(format: "%.1f", p.bestHeldOut) : "—", emphasis: true)
                stat("full", "\(p.bestHeldOutFull)/\(sim.trainSpec.held_out_seeds)")
            }

            if !sim.trainingCurve.isEmpty {
                Chart {
                    ForEach(Array(sim.trainingCurve.enumerated()), id: \.offset) { i, v in
                        LineMark(x: .value("iteration", i), y: .value("held-out return", v))
                    }
                }
                .frame(height: 90)
                .chartYAxis { AxisMarks(position: .leading) }
            }

            // Shown, but demoted: on a randomized env the trainer's own eval
            // selects for lucky draws, and a run whose held-out score is
            // climbing while this reads negative is normal, not broken.
            HStack(spacing: 12) {
                stat("train-eval", String(format: "%.1f", p.evalReward), dim: true)
                stat("σ", String(format: "%.3f", p.sigma), dim: true)
                stat("|Δθ|", String(format: "%.3f", p.updateNorm), dim: true)
                stat("α", String(format: "%.4f", p.stepSize), dim: true)
            }

            HStack(spacing: 8) {
                if p.running {
                    Button("Stop") { sim.stopTraining() }.controlSize(.small)
                    Button("Watch best") { sim.adoptTrainedPolicy() }
                        .controlSize(.small)
                        .help("Drive the robot with the best policy found so far")
                } else {
                    Button("Train again") { sim.startTraining() }.controlSize(.small)
                }
                Button("Save…") { savePolicy() }
                    .controlSize(.small)
                    .disabled(!p.bestHeldOut.isFinite)
            }

            if p.cancelled {
                Text("Cancelled at iteration \(p.iteration).")
                    .font(.system(size: 10)).foregroundStyle(.secondary)
            }
        }
    }

    // MARK: file IO

    private func loadPolicy() {
        #if os(macOS)
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.json]
        panel.allowsOtherFileTypes = true
        panel.message = "Choose a .vcadpolicy bundle"
        if panel.runModal() == .OK, let url = panel.url {
            sim.loadPolicy(from: url)
        }
        #endif
    }

    private func savePolicy() {
        #if os(macOS)
        let panel = NSSavePanel()
        panel.nameFieldStringValue = "policy.vcadpolicy"
        panel.allowedContentTypes = [.json]
        if panel.runModal() == .OK, let url = panel.url {
            sim.saveTrainedPolicy(to: url)
        }
        #endif
    }

    // MARK: small pieces

    @ViewBuilder
    private func section(_ title: String, @ViewBuilder content: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title.uppercased())
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(.tertiary)
            content()
        }
    }

    @ViewBuilder
    private func keyValue(_ k: String, _ v: String) -> some View {
        HStack {
            Text(k).font(.system(size: 11)).foregroundStyle(.secondary)
            Spacer(minLength: 8)
            Text(v).font(.system(size: 11, design: .monospaced))
        }
    }

    @ViewBuilder
    private func stat(_ label: String, _ value: String,
                      emphasis: Bool = false, dim: Bool = false) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text(label.uppercased())
                .font(.system(size: 8, weight: .semibold))
                .foregroundStyle(.tertiary)
            Text(value)
                .font(.system(size: emphasis ? 13 : 10,
                              weight: emphasis ? .semibold : .regular,
                              design: .monospaced))
                .foregroundStyle(dim ? AnyShapeStyle(.tertiary) : AnyShapeStyle(.primary))
        }
    }
}

/// An inline message with a tone. Kept local so the sim panel doesn't reach
/// into unrelated shell styling.
private struct Banner: View {
    enum Tone { case error, warning }
    let text: String
    let tone: Tone

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            Image(systemName: tone == .error ? "exclamationmark.triangle.fill" : "clock.badge.exclamationmark")
                .font(.system(size: 10))
            Text(text).font(.system(size: 10)).fixedSize(horizontal: false, vertical: true)
        }
        .foregroundStyle(tone == .error ? Color.red : Color.orange)
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background((tone == .error ? Color.red : Color.orange).opacity(0.1),
                    in: RoundedRectangle(cornerRadius: 6))
    }
}
