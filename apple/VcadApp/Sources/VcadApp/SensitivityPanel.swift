import SwiftUI
import CVcadFFI
import simd

// Parameter sensitivity in the inspector: for each named document parameter,
// how volume, mass and centre of mass move per unit change, from the
// kernel's differentiable seam (`vcad_document_sensitivity`) — analytic
// derivatives, not finite differences.

/// One parameter's derivatives, summed over solid parts.
struct ParameterSensitivity: Identifiable, Equatable {
    var id: String { name }
    let name: String
    let value: Double?
    /// d(volume)/dθ, mm³ per unit.
    let dVolume: Double
    /// d(mass)/dθ, g per unit.
    let dMass: Double
    /// d(centre of mass)/dθ of the whole, mm per unit.
    let dCentroid: SIMD3<Double>
    /// Why the gradient isn't available, if it isn't.
    let error: String?

    /// Parse `vcad_document_sensitivity` output. The whole-model centroid
    /// derivative combines parts by the quotient rule on Σmᵢcᵢ / Σmᵢ.
    static func parse(_ json: [String: Any]) -> [ParameterSensitivity] {
        (json["parameters"] as? [[String: Any]] ?? []).map { p in
            let name = p["name"] as? String ?? "?"
            let value = (p["value"] as? NSNumber)?.doubleValue
            if let err = p["error"] as? String {
                return ParameterSensitivity(name: name, value: value, dVolume: 0, dMass: 0, dCentroid: .zero, error: err)
            }
            func num(_ d: [String: Any], _ k: String) -> Double { (d[k] as? NSNumber)?.doubleValue ?? 0 }
            func vec(_ d: [String: Any], _ k: String) -> SIMD3<Double> {
                let a = (d[k] as? [NSNumber])?.map(\.doubleValue) ?? [0, 0, 0]
                return a.count == 3 ? SIMD3(a[0], a[1], a[2]) : .zero
            }
            let parts = p["parts"] as? [[String: Any]] ?? []
            var dV = 0.0, dM = 0.0, m = 0.0
            var mc = SIMD3<Double>.zero, dmc = SIMD3<Double>.zero
            for part in parts {
                let mi = num(part, "mass"), dmi = num(part, "d_mass")
                let ci = vec(part, "centroid"), dci = vec(part, "d_centroid")
                dV += num(part, "d_volume"); dM += dmi; m += mi
                mc += mi * ci
                dmc += dmi * ci + mi * dci
            }
            let dC = m > 0 ? (dmc * m - mc * dM) / (m * m) : .zero
            return ParameterSensitivity(name: name, value: value, dVolume: dV, dMass: dM, dCentroid: dC, error: nil)
        }
    }

    /// Run the seam on a document (off the main thread; several evaluations).
    nonisolated static func compute(documentJSON: Data, densityKgM3: Double) -> [ParameterSensitivity]? {
        let raw = documentJSON.withUnsafeBytes { buf in
            vcad_document_sensitivity(buf.bindMemory(to: UInt8.self).baseAddress, buf.count, densityKgM3)
        }
        guard let raw else { return nil }
        defer { vcad_cam_free(raw) }
        guard let obj = (try? JSONSerialization.jsonObject(with: Data(bytes: raw, count: strlen(raw)))) as? [String: Any]
        else { return nil }
        return parse(obj)
    }
}

/// The inspector section.
struct SensitivitySection: View {
    let model: EditorModel
    @State private var expanded = false
    @State private var rows: [ParameterSensitivity] = []
    @State private var computing = false
    @State private var computedFor: String?
    @State private var metric: Metric = .mass

    enum Metric: String, CaseIterable, Identifiable {
        case volume = "Volume", mass = "Mass", center = "Center"
        var id: String { rawValue }
    }

    /// Density for masses: the first root's material when the document
    /// defines one with a density, else aluminium.
    private var density: (value: Double, assumed: Bool) {
        let doc = model.documentJSON ?? [:]
        let roots = doc["roots"] as? [[String: Any]] ?? []
        let materials = doc["materials"] as? [String: Any] ?? [:]
        if let key = roots.first?["material"] as? String,
           let d = ((materials[key] as? [String: Any])?["density"] as? NSNumber)?.doubleValue, d > 0 {
            return (d, false)
        }
        return (2700, true)
    }

    var body: some View {
        DisclosureGroup("Sensitivity", isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: Theme.Space.s) {
                Picker("Metric", selection: $metric) {
                    ForEach(Metric.allCases) { Text($0.rawValue).tag($0) }
                }.pickerStyle(.segmented).labelsHidden()
                if computing {
                    ProgressView().controlSize(.small).frame(maxWidth: .infinity)
                } else if rows.isEmpty {
                    Text("No sensitivities yet.").font(.caption).foregroundStyle(.secondary)
                } else {
                    let scale = rows.map { magnitude($0) }.max() ?? 0
                    ForEach(rows) { row in SensitivityRow(row: row, metric: metric, scale: scale) }
                }
                HStack {
                    Text(density.assumed ? "Mass at 2700 kg/m³ (aluminium, assumed)" : "Mass at \(Int(density.value)) kg/m³")
                        .font(.caption2).foregroundStyle(.tertiary)
                    Spacer()
                    Button("Refresh") { refresh(force: true) }.controlSize(.small).disabled(computing)
                }
            }
            .padding(.top, Theme.Space.s)
        }
        .font(.callout)
        .onChange(of: expanded) { if expanded { refresh(force: false) } }
    }

    private func magnitude(_ r: ParameterSensitivity) -> Double {
        switch metric {
        case .volume: return abs(r.dVolume)
        case .mass: return abs(r.dMass)
        case .center: return simd_length(r.dCentroid)
        }
    }

    private func refresh(force: Bool) {
        guard let json = model.documentJSON, let data = try? JSONSerialization.data(withJSONObject: json) else { return }
        let key = String(data.hashValue)
        guard force || key != computedFor else { return }
        computing = true
        let density = self.density.value
        Task {
            let result = await Task.detached(priority: .userInitiated) {
                ParameterSensitivity.compute(documentJSON: data, densityKgM3: density)
            }.value
            rows = result ?? []
            computedFor = key
            computing = false
        }
    }
}

/// One parameter: name, signed derivative, and a sign/magnitude bar.
struct SensitivityRow: View {
    let row: ParameterSensitivity
    let metric: SensitivitySection.Metric
    /// Largest magnitude across rows, for the bar scale.
    let scale: Double

    private var signed: Double {
        switch metric {
        case .volume: return row.dVolume
        case .mass: return row.dMass
        // Centre: the dominant axis's direction sets the sign.
        case .center:
            let c = row.dCentroid
            let m = [c.x, c.y, c.z].max { abs($0) < abs($1) } ?? 0
            return (m < 0 ? -1 : 1) * simd_length(c)
        }
    }

    private var text: String {
        switch metric {
        case .volume: return String(format: "%+.3g mm³/unit", row.dVolume)
        case .mass: return String(format: "%+.3g g/unit", row.dMass)
        case .center:
            let c = row.dCentroid
            return String(format: "(%+.2g, %+.2g, %+.2g) mm/unit", c.x, c.y, c.z)
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline) {
                Text(row.name).font(.callout.monospaced())
                Spacer(minLength: Theme.Space.s)
                if row.error == nil {
                    Text(text).font(.caption).monospacedDigit().foregroundStyle(.secondary)
                } else {
                    Image(systemName: "exclamationmark.triangle").foregroundStyle(.orange).help(row.error ?? "")
                }
            }
            if row.error == nil {
                GeometryReader { geo in
                    let half = geo.size.width / 2
                    let frac = scale > 0 ? min(1, abs(signed) / scale) : 0
                    ZStack(alignment: .leading) {
                        Capsule().fill(.quaternary)
                        Rectangle().fill(.separator).frame(width: 1).offset(x: half)
                        Capsule()
                            .fill(signed >= 0 ? Color.accentColor : Color.orange)
                            .frame(width: max(2, half * frac))
                            .offset(x: signed >= 0 ? half : half - half * frac)
                    }
                }
                .frame(height: 5)
                .accessibilityLabel("\(row.name) sensitivity \(text)")
            }
        }
        .help(row.error ?? "\(row.name) = \(row.value.map { String(format: "%g", $0) } ?? "formula")")
    }
}
