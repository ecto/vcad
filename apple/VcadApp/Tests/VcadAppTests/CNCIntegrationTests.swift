import XCTest
@testable import VcadApp

/// The seams between the four Manufacture packages.
///
/// Each of these is a joint that nothing owned before the packages were
/// merged: the machine's gate inside the job's gate, the probe's measurement
/// inside the job's placement, ncSender's hold on the controller inside the
/// native sender's Run button, and the menu bar's enabled state inside the
/// buttons' own predicates.
@MainActor
final class CNCIntegrationTests: XCTestCase {

    private func built(_ cnc: CNCWorkspace) async {
        cnc.build()
        let deadline = Date().addingTimeInterval(60)
        while cnc.generating && Date() < deadline { try? await Task.sleep(for: .milliseconds(20)) }
        XCTAssertFalse(cnc.generating)
    }

    /// A job that will run, in the simulator, with every warning acknowledged.
    private func runnable() async -> CNCWorkspace {
        let cnc = CNCWorkspace()
        await built(cnc)
        for warning in cnc.warnings { cnc.acknowledge(warning.id, on: true) }
        cnc.machine.connect(simulated: true)
        cnc.setupConfirmed = true
        return cnc
    }

    // MARK: - The machine's half of the gate reaches runBlocker

    /// `runBlocker` used to answer only the job's half; the machine bar
    /// combined the two itself, so every other caller — the readiness list,
    /// the menu, anything new — saw a job as ready when the machine said it
    /// would run off the end of the table.
    func testMachineBlockerReachesRunBlocker() async throws {
        let cnc = await runnable()
        defer { cnc.machine.disconnect() }
        XCTAssertNil(cnc.runBlocker, "the job's own half is clear")

        // A hard limit: the controller stopped mid-step, so every coordinate
        // on screen is a guess. Nothing about the *job* changed.
        cnc.machine.simulate(alarm: 1)

        // The machine's half sees it…
        let machineBlocker = try XCTUnwrap(cncMachineBlocker(cnc),
                                           "a position-lost alarm is the machine refusing")
        XCTAssertTrue(machineBlocker.contains("ALARM:1"), machineBlocker)
        // …and so does the one gate everything else reads, in the machine's own
        // words rather than a generic "waiting for Idle".
        let blocker = try XCTUnwrap(cnc.runBlocker)
        XCTAssertTrue(blocker.contains("ALARM:1"), blocker)
        XCTAssertTrue(blocker.contains("re-home"), blocker)
        XCTAssertFalse(CNCCommand.runJob.isEnabled(cnc))
        cnc.startJob()
        XCTAssertFalse(cnc.machine.active, "and the gate is closed, not just labelled")
    }

    /// The final clause does not over-fire: a machine finding that is a
    /// *warning* — the Simulator's fictional travel, soft limits off — leaves
    /// Run open, because `cncMachineBlocker` only ever returns a blocking one.
    func testANonBlockingMachineFindingDoesNotCloseTheGate() async throws {
        let cnc = await runnable()
        defer { cnc.machine.disconnect() }
        let findings = cncMachineFindings(cnc)
        XCTAssertFalse(findings.isEmpty, "the Simulator has plenty to say")
        XCTAssertTrue(findings.allSatisfy { !$0.blocking },
                      "none of it a refusal: \(findings.filter(\.blocking).map(\.text))")
        XCTAssertNil(cncMachineBlocker(cnc))
        XCTAssertNil(cnc.runBlocker)
    }

    /// Mutation check for the above: the job's own half, asked alone, still
    /// says yes. That is exactly what made the old split dangerous — every
    /// caller but the machine bar saw a job as ready while the controller was
    /// sitting in an alarm.
    func testMachineBlockerIsWhatMakesTheDifference() async throws {
        let cnc = await runnable()
        defer { cnc.machine.disconnect() }
        cnc.machine.simulate(alarm: 1)
        XCTAssertNotNil(cnc.runBlocker)
        XCTAssertTrue(cnc.blockers.isEmpty, "the toolpath is fine")
        XCTAssertTrue(cnc.unacknowledgedWarnings.isEmpty)
        XCTAssertTrue(cnc.setupConfirmed)
        XCTAssertNotNil(cnc.jobCode, "there is still G-code — it is the machine that says no")
    }

    /// A job that leaves a *real* machine's travel is refused; the same job on
    /// the Simulator is not, because the Simulator has no table.
    ///
    /// Blocking on the Simulator's fictional travel made the Simulator unable
    /// to run anything at all — which is the one thing it is for — and the
    /// sentence it produced named an AnoleX the operator was not standing in
    /// front of. The warning is still said, with its numbers; only the refusal
    /// is withheld.
    func testTravelRefusalIsAboutARealTableOrItIsNotARefusal() throws {
        let short = CNCMachineSettings.parse("$20=1\n$21=1\n$22=1\n$23=3\n$27=3\n$130=10\n$131=10\n$132=10")
        var real = CNCMachineProfile(settings: short)
        real.workOffset = CNCVector()
        real.homed = true
        let job = try XCTUnwrap(CNCEnvelopeBox.around(
            moves: [[0, 0, 0], [43, 30, -1]], toolRadius: 1))

        let onMetal = CNCMachineCheck.travelFindings(job: job, profile: real)
        let refusal = try XCTUnwrap(onMetal.first(where: \.blocking)?.text,
                                    "43 mm of job does not fit 10 mm of travel")
        XCTAssertTrue(refusal.contains("past the"), refusal)

        var simulated = real
        simulated.simulated = true
        let onScreen = CNCMachineCheck.travelFindings(job: job, profile: simulated)
        XCTAssertEqual(onScreen.count, onMetal.count, "the same findings, one for one")
        XCTAssertTrue(onScreen.allSatisfy { !$0.blocking }, "none of them a refusal")
        XCTAssertTrue(onScreen.contains { $0.text.contains("Simulator: no real table") },
                      "…and each says why: \(onScreen.map(\.text))")
    }

    // MARK: - Skew → placement, with the sign stated once

    /// `CNCSkewProbe` documents the convention: a blank turned +10° takes a
    /// +10° job, because the job is being laid on the blank as it sits, not
    /// straightened. One click has to mean exactly that.
    func testMeasuredSkewTurnsTheJobTheSameWay() {
        let cnc = CNCWorkspace()
        XCTAssertNil(cnc.measuredSkewDegrees)
        XCTAssertFalse(cnc.canApplyMeasuredSkew)
        XCTAssertFalse(cnc.applyMeasuredSkew(), "nothing measured, nothing to apply")

        cnc.machine.connect(simulated: true)
        defer { cnc.machine.disconnect() }
        cnc.machine.setSkew(10)
        XCTAssertEqual(cnc.measuredSkewDegrees, 10)
        XCTAssertTrue(cnc.canApplyMeasuredSkew)
        XCTAssertTrue(cnc.applyMeasuredSkew())
        XCTAssertEqual(cnc.placement.rotationDeg, 10, accuracy: 1e-9,
                       "a +10° blank takes a +10° job — the same sign, not its negative")
        XCTAssertEqual(cnc.placement.rotationDeg,
                       CNCSkewProbe.placementRotationDegrees(forSkew: 10),
                       "and it is CNCSkewProbe that says so")
        XCTAssertFalse(cnc.canApplyMeasuredSkew, "already applied")

        // The other way round, so a sign flip cannot pass by symmetry.
        cnc.machine.setSkew(-3.5)
        XCTAssertTrue(cnc.applyMeasuredSkew())
        XCTAssertEqual(cnc.placement.rotationDeg, -3.5, accuracy: 1e-9)
    }

    /// The probe's own geometry, for the sign the placement inherits: a blank
    /// rotated counter-clockwise leans its left edge so the upper station sits
    /// further left, i.e. Δx negative as Δy grows.
    func testSkewProbeSignMatchesItsDocumentedConvention() throws {
        let skew = try XCTUnwrap(CNCSkewProbe.skewDegrees(
            axis: "X",
            a: SIMD2<Double>(0, 0),
            b: SIMD2<Double>(-tan(10 * .pi / 180) * 20, 20)))
        XCTAssertEqual(skew, 10, accuracy: 1e-6)
        XCTAssertEqual(CNCSkewProbe.placementRotationDegrees(forSkew: skew), skew)
    }

    // MARK: - Two senders, one controller

    /// ncSender holds the Anolex's telnet session. If the built-in sender is
    /// pointed at the same address, whichever connects second either fails or
    /// interleaves commands into the same stream — so Run must refuse, and say
    /// which sender is holding it.
    func testNcSenderContentionStopsTheNativeRun() async throws {
        let cnc = await runnable()
        defer { cnc.machine.disconnect() }
        XCTAssertNil(cnc.runBlocker)

        cnc.machine.host = "192.168.2.226"
        cnc.machine.port = "23"
        cnc.ncSender.simulate(controllerAddress: "192.168.2.226:23",
                              nativeHost: cnc.machine.host, nativePort: cnc.machine.port)

        let blocker = try XCTUnwrap(cnc.runBlocker)
        XCTAssertTrue(blocker.contains("ncSender is holding"), blocker)
        XCTAssertTrue(blocker.contains("Only one sender may hold the controller"), blocker)
        XCTAssertFalse(CNCCommand.runJob.isEnabled(cnc))

        // And the gate really is closed, not merely labelled.
        cnc.startJob()
        XCTAssertFalse(cnc.machine.active)
    }

    /// Mutation check: a sender holding a *different* controller is not
    /// contention, and must not block. Without this the guard could be "always
    /// block once a probe has run" and still pass the test above.
    func testADifferentControllerIsNotContention() async throws {
        let cnc = await runnable()
        defer { cnc.machine.disconnect() }
        cnc.machine.host = "192.168.2.226"
        cnc.ncSender.simulate(controllerAddress: "192.168.2.99:23",
                              nativeHost: cnc.machine.host, nativePort: cnc.machine.port)
        XCTAssertNil(cnc.senderContention)
        XCTAssertNil(cnc.runBlocker)
        XCTAssertTrue(CNCCommand.runJob.isEnabled(cnc))
    }

    // MARK: - The menu item and the button ask the same question

    /// Every command's enabled state, under three job states. The point is not
    /// the individual answers — it is that the menu bar has no predicate of
    /// its own to drift from the buttons'.
    func testMenuEnabledStateMatchesTheButtonPredicate() async throws {
        // Unbuilt: no job, no outline, nothing connected.
        let unbuilt = CNCWorkspace()
        assertEnabled(unbuilt, [
            .outlineFromModel: false,   // no document on screen
            .importOutlineDXF: true,
            .buildJob: true,
            .verifyJob: false,          // nothing to replay against
            .exportJob: false,          // no G-code
            .sendToNcSender: false,     // not built, not connected
            .traceBounds: false,        // no controller
            .probe: false,
            .connect: true,
            .runJob: false,
        ])

        // Passing: built, warnings acknowledged, simulator connected.
        let passing = await runnable()
        defer { passing.machine.disconnect() }
        assertEnabled(passing, [
            .outlineFromModel: false,
            .importOutlineDXF: true,
            .buildJob: true,
            .verifyJob: false,
            .exportJob: true,           // there is G-code to write
            .sendToNcSender: false,     // ncSender itself is not connected
            .traceBounds: true,
            .probe: true,
            .connect: true,
            .runJob: true,
        ])

        // Blocked: a real refusal from the oracle, with an outline behind it.
        let blocked = try await blockedJob()
        XCTAssertFalse(blocked.blockers.isEmpty, "this fixture is meant to be refused")
        assertEnabled(blocked, [
            .outlineFromModel: false,
            .importOutlineDXF: true,
            .buildJob: true,
            .verifyJob: true,           // there is an outline to replay against
            .exportJob: false,          // a refused job has no G-code at all
            .sendToNcSender: false,
            .traceBounds: false,
            .probe: false,
            .connect: true,
            .runJob: false,
        ])
    }

    /// Mutation check for the parity test: if `exportJob` stopped asking
    /// whether there is any G-code — the one predicate that carries the whole
    /// fail-closed gate — the blocked case would flip. Asserted here so the
    /// table above cannot quietly become a list of `true`s.
    func testExportIsTheGateNotALabel() async throws {
        let blocked = try await blockedJob()
        XCTAssertNil(blocked.jobCode, "a refused job has no G-code")
        XCTAssertFalse(CNCCommand.exportJob.isEnabled(blocked))
        XCTAssertFalse(blocked.export(job: true), "and nothing to write if asked anyway")

        let passing = await runnable()
        defer { passing.machine.disconnect() }
        XCTAssertNotNil(passing.jobCode)
        XCTAssertTrue(CNCCommand.exportJob.isEnabled(passing))
    }

    /// Streaming freezes the job: nothing that edits or rebuilds it may run
    /// while the controller is taking lines.
    func testNothingEditsTheJobWhileItIsStreaming() async throws {
        let cnc = await runnable()
        defer { cnc.machine.disconnect() }
        cnc.startJob()
        XCTAssertTrue(cnc.machine.active)
        for command in [CNCCommand.buildJob, .verifyJob, .importOutlineDXF,
                        .outlineFromModel, .connect] {
            XCTAssertFalse(command.isEnabled(cnc), "\(command.rawValue) must not run mid-job")
        }
    }

    /// Every command has a shortcut, every shortcut is ⌘ with a second
    /// modifier, and no two share a key. A bare-letter shortcut would fire
    /// while a number field has focus.
    func testShortcutsAreDistinctAndNeverBareLetters() {
        var seen: Set<String> = []
        for command in CNCCommand.allCases {
            let shortcut = command.shortcut
            XCTAssertNotNil(shortcut, "\(command.rawValue) has no shortcut")
            guard let shortcut else { continue }
            XCTAssertTrue(shortcut.modifiers.contains(.command), "\(command.rawValue) is not a ⌘ shortcut")
            XCTAssertTrue(shortcut.modifiers.contains(.option) || shortcut.modifiers.contains(.control)
                            || shortcut.modifiers.contains(.shift),
                          "\(command.rawValue) is ⌘ alone")
            XCTAssertNotEqual(shortcut.key, .return, "never Return — AppKit makes it the default button")
            let key = "\(shortcut.modifiers.rawValue)-\(shortcut.key.character)"
            XCTAssertFalse(seen.contains(key), "\(command.rawValue) collides on \(key)")
            seen.insert(key)
        }
    }

    // MARK: - ncSender preferences fold into Prefs

    /// `NcSenderPrefs` was a second preferences enum under the same
    /// `vcad.prefs.*` namespace. Folding it in has to keep the keys, or every
    /// operator's stored sender address disappears on upgrade.
    func testSenderPrefsKeptTheirKeys() {
        XCTAssertEqual(Prefs.ncSenderURLKey, "vcad.prefs.ncsender.url")
        XCTAssertEqual(Prefs.cameraURLKey, "vcad.prefs.camera.url")
        XCTAssertEqual(Prefs.cameraIntervalKey, "vcad.prefs.camera.interval")
        XCTAssertEqual(Prefs.ncSenderDefaultURL, NcSenderClient.defaultBaseURL,
                       "Prefs spells the default out because Theme.swift is shared with visionOS; the two must agree")
    }

    // MARK: - helpers

    private func assertEnabled(_ cnc: CNCWorkspace, _ expected: [CNCCommand: Bool],
                               file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(Set(expected.keys), Set(CNCCommand.allCases),
                       "every command needs an expectation", file: file, line: line)
        for (command, want) in expected {
            XCTAssertEqual(command.isEnabled(cnc), want,
                           "\(command.rawValue) should be \(want ? "enabled" : "disabled")",
                           file: file, line: line)
        }
    }

    /// The blocked fixture: the stator outline cut deeper than the blank is
    /// thick, on a bare machine bed. The oracle refuses it, so it has no
    /// G-code — which is the state most of these predicates turn on.
    private func blockedJob() async throws -> CNCWorkspace {
        let cnc = CNCWorkspace()
        cnc.toolDiameter = 2
        cnc.stockThickness = 0.8
        try cnc.importOutline(try CNCOutline.parseDXF(try CNCJobTests.statorDXF(),
                                                      name: "stator-outline.dxf"))
        for operation in cnc.operations {
            cnc.select(.operation(operation.id))
            cnc.setup.depth = 1.0
            cnc.setup.bottomAllowance = 0
            cnc.setup.stepdown = 0.17
        }
        await built(cnc)
        return cnc
    }
}
