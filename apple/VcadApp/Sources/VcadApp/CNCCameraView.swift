import SwiftUI

// The camera tile. One still, its age, and one button to take another.
//
// The age is the point. A tile showing a frame from four minutes ago looks
// exactly like a live one, and on 2026-09-17 the camera was the only thing
// telling the operator whether the cutter was still in the material.

struct CNCCameraTile: View {
    @Bindable var camera: CNCCameraModel
    /// Shown instead of a live frame in snapshots and previews.
    var placeholder: NSImage?
    var onClose: (() -> Void)?

    @State private var entry = ""
    @State private var editing = false

    private var shown: NSImage? { camera.frame ?? placeholder }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.s) {
            PanelHeader(title: "Camera", systemImage: "video", onClose: onClose)
            frame
            status
            controls
            if editing || !camera.hasURL { entryField } else { addressRow }
        }
        .padding(Theme.Space.m)
        .frame(minWidth: 260)
    }

    // MARK: the frame

    private var frame: some View {
        ZStack {
            RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous)
                .fill(.quaternary)
            if let shown {
                Image(nsImage: shown)
                    .resizable().aspectRatio(contentMode: .fit)
                    .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.control, style: .continuous))
                    .accessibilityLabel("Camera frame, \(camera.ageLabel)")
            } else {
                VStack(spacing: Theme.Space.xs) {
                    Image(systemName: "video.slash").font(.title2).foregroundStyle(.tertiary)
                    Text("no frame").font(.callout).foregroundStyle(.secondary)
                }
            }
        }
        .frame(height: 168)
        .frame(maxWidth: .infinity)
    }

    // MARK: what it is doing

    private var status: some View {
        TimelineView(.periodic(from: .now, by: 1)) { _ in
            HStack(spacing: 6) {
                Circle().fill(dotColor).frame(width: 7, height: 7)
                Text(camera.frame == nil ? "no frame" : camera.ageLabel)
                    .font(.callout.weight(.medium)).monospacedDigit()
                Spacer(minLength: 0)
                if camera.grabbing { ProgressView().controlSize(.small) }
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Camera, \(camera.frame == nil ? "no frame" : camera.ageLabel)")
        }
    }

    private var dotColor: Color {
        switch camera.state {
        case .live: return camera.frame == nil ? .secondary : .green
        case .waiting: return .yellow
        case .failed: return .orange
        case .off: return .secondary
        }
    }

    @ViewBuilder private var controls: some View {
        HStack(spacing: Theme.Space.s) {
            Button(camera.running ? "Stop" : "Watch") {
                camera.running ? camera.stop() : camera.start()
            }.disabled(!camera.hasURL)
            Button("Grab frame now") { Task { await camera.grabNow() } }
                .disabled(!camera.hasURL || camera.grabbing)
            Spacer()
        }.controlSize(.small)
        if case .failed(let detail) = camera.state {
            Text(detail).font(.caption).foregroundStyle(Color.orange)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    // MARK: the URL, which is a secret

    private var addressRow: some View {
        HStack {
            Text(camera.maskedURL).font(.caption.monospaced()).foregroundStyle(.secondary)
                .lineLimit(1).truncationMode(.middle)
            Spacer()
            Button("Change") { entry = ""; editing = true }.buttonStyle(.borderless).font(.caption)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("Camera address, hidden")
    }

    private var entryField: some View {
        VStack(alignment: .leading, spacing: Theme.Space.xs) {
            // A camera URL carries `user:pass@`, so it is entered like a
            // password and masked the moment it is committed.
            SecureField("rtsps://…/stream", text: $entry)
                .textFieldStyle(.roundedBorder)
                .accessibilityLabel("Camera URL")
                .onSubmit(commit)
            HStack {
                Button("Save") { commit() }.disabled(entry.trimmingCharacters(in: .whitespaces).isEmpty)
                if camera.hasURL { Button("Cancel") { entry = ""; editing = false }.buttonStyle(.borderless) }
                Spacer()
                Picker("Every", selection: Binding(get: { camera.interval }, set: { camera.setInterval($0) })) {
                    Text("2 s").tag(2.0); Text("3 s").tag(3.0); Text("5 s").tag(5.0)
                }.labelsHidden().fixedSize()
            }.controlSize(.small)
            Text("Stored on this Mac and never written to a log. RTSP needs ffmpeg on PATH; an HTTP snapshot URL needs nothing.")
                .font(.caption2).foregroundStyle(.tertiary).fixedSize(horizontal: false, vertical: true)
        }
    }

    private func commit() {
        let text = entry.trimmingCharacters(in: .whitespaces)
        guard !text.isEmpty else { return }
        camera.setURL(text)
        entry = ""
        editing = false
    }
}
