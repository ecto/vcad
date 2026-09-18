import Foundation
#if canImport(AppKit)
import AppKit
#endif

// What the app is doing, as a file.
//
// Friction-log item 31: *"Verifying the app blind is slow: the editor window
// is borderless, so it has no accessibility window, no Window-menu entry and
// no title to query. A `VCAD_STATUS` dump (document, workspace, solve state)
// on a signal or a debug menu item would have saved an hour here."*
//
// So: `kill -USR1 <pid>` (or Debug ▸ Dump Status) writes one JSON document and
// prints its path to stderr. It is a *report*, never a control surface —
// nothing here moves a machine, changes a setting or starts a build.

/// One snapshot of the running app.
///
/// Encoded by hand into `[String: Any]` rather than through `Codable` because
/// half of what is worth dumping is a computed property on an `@Observable`
/// class, and a parallel set of `Codable` mirrors would be one more thing to
/// keep in step with the model.
enum VcadStatus {
    /// Where the dump goes. `VCAD_STATUS` names a file; without it the dump
    /// lands beside the other per-run temporaries, under a name carrying the
    /// pid so two instances cannot overwrite each other's.
    static func destination(environment: [String: String] = ProcessInfo.processInfo.environment,
                            pid: Int32 = ProcessInfo.processInfo.processIdentifier) -> URL {
        if let path = environment["VCAD_STATUS"]?.trimmingCharacters(in: .whitespaces), !path.isEmpty {
            return URL(fileURLWithPath: (path as NSString).expandingTildeInPath)
        }
        return FileManager.default.temporaryDirectory
            .appendingPathComponent("vcad-status-\(pid).json")
    }

    /// The whole snapshot.
    @MainActor static func snapshot(_ model: EditorModel, at now: Date = Date()) -> [String: Any] {
        let cnc = model.cnc
        return [
            "at": ISO8601DateFormatter().string(from: now),
            "pid": Int(ProcessInfo.processInfo.processIdentifier),
            "document": document(model),
            "workspace": model.workspace.rawValue,
            "solve": solve(model),
            "job": job(cnc),
            "machine": machine(cnc),
            "ncSender": sender(cnc),
        ]
    }

    @MainActor private static func document(_ model: EditorModel) -> [String: Any] {
        var out: [String: Any] = [
            "name": model.documentName,
            "dirty": model.documentDirty,
            "parts": model.partCount,
            "triangles": model.triangleCount,
        ]
        // The path is the thing a script needs to know it is looking at the
        // right document; a sandbox has none, and says so by its absence.
        if let url = model.documentURL { out["path"] = url.path }
        return out
    }

    @MainActor private static func solve(_ model: EditorModel) -> [String: Any] {
        [
            "solving": model.solving,
            "done": model.solveProgress.done,
            "total": model.solveProgress.total,
            "hasGeometry": model.usesDocumentTree,
        ]
    }

    @MainActor private static func job(_ cnc: CNCWorkspace) -> [String: Any] {
        var out: [String: Any] = [
            "source": cnc.usesImportedProgram ? "imported" : "generated",
            "name": cnc.jobTitle,
            "mode": cnc.mode.rawValue,
            "operations": cnc.operations.map { operation in
                [
                    "name": operation.name,
                    "kind": operation.setup.kind.rawValue,
                    "depthMm": operation.setup.depth,
                    "seconds": operation.seconds,
                    "built": !operation.ranges.isEmpty,
                ] as [String: Any]
            },
            "current": cnc.jobCurrent,
            "building": cnc.generating,
            "verified": cnc.verified,
            // The gate, in one line. `false` here with no blockers means the
            // job was built and passed; there is no third state.
            "blocked": !cnc.blockers.isEmpty,
            "blockers": cnc.blockers.map(\.text),
            "warnings": cnc.warnings.map(\.text),
            "unacknowledgedWarnings": cnc.unacknowledgedWarnings.map(\.text),
            "hasGcode": cnc.jobCode != nil,
            "durationS": cnc.jobDuration,
            "toolDiameterMm": cnc.toolDiameter,
            // Why Run Job will not go, or `nil` when it will.
            "runBlocker": cnc.runBlocker as Any,
            "headline": headline(cnc),
        ]
        if let outline = cnc.outline {
            out["outline"] = ["name": outline.name, "holes": outline.holes.count] as [String: Any]
        }
        return out
    }

    /// The verification verdict in the words the job outline shows, so a dump
    /// and a screenshot cannot disagree.
    @MainActor private static func headline(_ cnc: CNCWorkspace) -> String {
        if !cnc.jobCurrent { return "Not built" }
        if !cnc.blockers.isEmpty { return "Refused · \(counted(cnc.blockers.count, "reason"))" }
        if !cnc.verified { return "Not verified" }
        let pending = cnc.unacknowledgedWarnings.count
        return pending > 0 ? "\(counted(pending, "warning")) to acknowledge" : "Verified against the part"
    }

    @MainActor private static func machine(_ cnc: CNCWorkspace) -> [String: Any] {
        let machine = cnc.machine
        var out: [String: Any] = [
            "connected": machine.connected,
            "simulated": machine.demo,
            "host": machine.host,
            "port": machine.port,
            "state": machine.status.state,
            "fresh": machine.status.isFresh,
            "faulted": machine.faulted,
            "workspace": machine.workspace,
            "streaming": machine.active,
            "summary": machine.summary,
            "setupConfirmed": cnc.setupConfirmed,
        ]
        // A profile exists only once `$$` has been read. Absent means unknown,
        // which is not the same as "fine" — so it is absent, not defaulted.
        if let profile = machine.profile {
            out["profile"] = [
                "homed": profile.homed,
                "softLimits": profile.softLimits,
                "hardLimits": profile.hardLimits,
                "travelMm": profile.travels.map(\.length),
                "settingsRead": profile.settings.numbers.count,
                "baseline": profile.baselineName as Any,
                "changedSinceBaseline": profile.baselineDiff.map(\.summary),
                "skewDegrees": profile.skewDegrees as Any,
            ] as [String: Any]
        }
        if let alarm = machine.alarm { out["alarm"] = ["code": alarm.rawValue, "text": alarm.text] as [String: Any] }
        return out
    }

    @MainActor private static func sender(_ cnc: CNCWorkspace) -> [String: Any] {
        let sender = cnc.ncSender
        var out: [String: Any] = [
            // The sender's own address is a plain host; the *camera* URL is a
            // credential and is never dumped, only whether one is set.
            "url": sender.baseURLText,
            "polling": sender.lastPoll != nil,
            "blockers": cncSenderBlockers(cnc),
            "cameraConfigured": cnc.camera.hasURL,
            "cameraWatching": cnc.camera.running,
        ]
        if let probe = sender.probe {
            out["probe"] = [
                "reachable": probe.reachable,
                "version": probe.version as Any,
                "controller": probe.controllerAddress as Any,
                "contention": probe.contention as Any,
            ] as [String: Any]
        }
        if let state = sender.machine {
            out["machine"] = ["status": state.status, "homed": state.homed] as [String: Any]
        }
        if let job = sender.job {
            out["job"] = [
                "filename": job.filename,
                "line": job.currentLine,
                "lines": job.totalLines,
                "running": job.isRunning,
            ] as [String: Any]
        }
        if let error = sender.lastError { out["lastError"] = error }
        return out
    }

    /// Write the snapshot and say where it went. Returns the path so a menu
    /// item can reveal it; stderr gets it either way, because a dump nobody
    /// can find is not a dump.
    @discardableResult
    @MainActor static func dump(_ model: EditorModel, to url: URL? = nil) -> URL? {
        let target = url ?? destination()
        do {
            let data = try JSONSerialization.data(withJSONObject: snapshot(model),
                                                  options: [.prettyPrinted, .sortedKeys])
            try data.write(to: target, options: .atomic)
            FileHandle.standardError.write(Data("[VCAD_STATUS] \(target.path)\n".utf8))
            return target
        } catch {
            FileHandle.standardError.write(Data("[VCAD_STATUS] could not write \(target.path): \(error)\n".utf8))
            return nil
        }
    }

    // MARK: - The signal

    /// `SIGUSR1` arrives on a signal handler, where almost nothing is legal to
    /// call. `DispatchSource` moves it to a normal queue, which is why the
    /// handler itself does nothing but let the source fire.
    @MainActor private static var source: DispatchSourceSignal?

    @MainActor static func installSignalHandler(_ model: EditorModel) {
        guard source == nil else { return }
        signal(SIGUSR1, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
        source.setEventHandler {
            MainActor.assumeIsolated { _ = dump(model) }
        }
        source.resume()
        Self.source = source
        FileHandle.standardError.write(Data(
            "[VCAD_STATUS] kill -USR1 \(ProcessInfo.processInfo.processIdentifier) writes \(destination().path)\n".utf8))
    }
}
