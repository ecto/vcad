import CIrys
import Foundation
import Observation
import SwiftUI

/// Path-tracer settings, app-wide and remembered between launches. One shared
/// instance: the Settings window, the Pose panel's popover and the engine all
/// read and write the same values, so they cannot drift.
@MainActor @Observable
final class RendererSettings {
    static let shared = RendererSettings()
    static let key = "vcad.pathtrace"

    enum Background: String, Codable, CaseIterable, Identifiable {
        /// Transparent when released over the desktop, studio in a window.
        case auto, studio, transparent
        var id: String { rawValue }
        var label: String {
            switch self {
            case .auto: return "Automatic"
            case .studio: return "Studio"
            case .transparent: return "Transparent"
            }
        }
    }

    struct Values: Codable, Equatable {
        var background: Background = .auto
        /// Floor under the model (studio background only): catches shadows.
        var floor = true
        /// Trace resolution relative to the view's points (2 = Retina).
        var resolution: Double = 1
        /// Stop refining at this many samples per pixel.
        var maxSamples: Int = 4096
        /// Path length cap (light bounces).
        var bounces: Int = 7
        var exposure: Double = 1
        var sky: Double = 1
        var sun: Double = 1
        var sunAzimuth: Double = 50.7
        var sunElevation: Double = 44.4
        var sunSize: Double = 3.4
        var floorAlbedo: Double = 0.31
    }

    var values: Values {
        didSet { if values != oldValue { save() } }
    }

    private init() {
        if let d = UserDefaults.standard.data(forKey: Self.key),
           let v = try? JSONDecoder().decode(Values.self, from: d) {
            values = v
        } else {
            values = Values()
        }
    }

    private func save() {
        if let d = try? JSONEncoder().encode(values) { UserDefaults.standard.set(d, forKey: Self.key) }
    }

    func reset() { values = Values() }

    /// Whether the background is transparent for a viewport that is (or is
    /// not) released over the desktop.
    func transparent(released: Bool) -> Bool {
        switch values.background {
        case .auto: return released
        case .studio: return false
        case .transparent: return true
        }
    }

    /// What the scene must be BUILT with: changing any of it rebuilds the
    /// irys scene (geometry is re-uploaded). Everything else is cheap.
    struct Build: Equatable {
        var lighting: IrysLighting
        var floor: Bool
        static func == (a: Build, b: Build) -> Bool {
            a.floor == b.floor && a.lighting.sky == b.lighting.sky && a.lighting.sun == b.lighting.sun
                && a.lighting.sun_azimuth_deg == b.lighting.sun_azimuth_deg
                && a.lighting.sun_elevation_deg == b.lighting.sun_elevation_deg
                && a.lighting.sun_size_deg == b.lighting.sun_size_deg
                && a.lighting.floor_albedo == b.lighting.floor_albedo
        }
    }

    func build(released: Bool) -> Build {
        let v = values
        return Build(lighting: IrysLighting(sky: Float(v.sky), sun: Float(v.sun),
                                            sun_azimuth_deg: Float(v.sunAzimuth),
                                            sun_elevation_deg: Float(v.sunElevation),
                                            sun_size_deg: Float(v.sunSize),
                                            floor_albedo: Float(v.floorAlbedo)),
                     floor: v.floor && !transparent(released: released))
    }

    /// Per-frame options: changing these only restarts accumulation.
    struct Render: Equatable {
        var transparent: Bool
        var exposure: Float
        var bounces: UInt32
    }

    func render(released: Bool) -> Render {
        Render(transparent: transparent(released: released), exposure: Float(values.exposure),
               bounces: UInt32(max(1, values.bounces)))
    }
}

/// The renderer's controls: used by Settings ▸ Renderer and the Pose panel's
/// popover.
struct RendererSettingsForm: View {
    @Bindable var settings = RendererSettings.shared

    var body: some View {
        Section("Path tracing") {
            Picker("Background", selection: $settings.values.background) {
                ForEach(RendererSettings.Background.allCases) { Text($0.label).tag($0) }
            }
            Toggle("Floor (studio background)", isOn: $settings.values.floor)
            Picker("Resolution", selection: $settings.values.resolution) {
                Text("Half").tag(0.5)
                Text("Standard").tag(1.0)
                Text("Retina").tag(2.0)
            }
            Picker("Stop at", selection: $settings.values.maxSamples) {
                ForEach([256, 1024, 4096, 16384], id: \.self) { Text("\($0) spp").tag($0) }
            }
            Stepper("Bounces: \(settings.values.bounces)", value: $settings.values.bounces, in: 1...32)
            LabeledSlider(title: "Exposure", value: $settings.values.exposure, range: 0.25...4, format: "%.2f×")
        }
        Section("Lighting") {
            LabeledSlider(title: "Sky", value: $settings.values.sky, range: 0...3, format: "%.2f")
            LabeledSlider(title: "Sun", value: $settings.values.sun, range: 0...3, format: "%.2f")
            LabeledSlider(title: "Sun azimuth", value: $settings.values.sunAzimuth, range: -180...180, format: "%.0f°")
            LabeledSlider(title: "Sun elevation", value: $settings.values.sunElevation, range: 5...89, format: "%.0f°")
            LabeledSlider(title: "Shadow softness", value: $settings.values.sunSize, range: 0.5...15, format: "%.1f°")
            LabeledSlider(title: "Floor albedo", value: $settings.values.floorAlbedo, range: 0...1, format: "%.2f")
            Button("Restore Defaults") { settings.reset() }
        }
    }
}

private struct LabeledSlider: View {
    let title: String
    @Binding var value: Double
    let range: ClosedRange<Double>
    let format: String

    var body: some View {
        LabeledContent(title) {
            HStack {
                Slider(value: $value, in: range).controlSize(.small)
                Text(String(format: format, value)).font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary).frame(width: 48, alignment: .trailing)
            }
        }
    }
}
