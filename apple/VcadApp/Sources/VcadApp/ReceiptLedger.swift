import SwiftUI

/// The always-on cross-domain Receipt — the vision's moat made literal. Three
/// gating checks (mechanical min-wall · electrical copper · sheet-metal bracket)
/// plus an informational manufacturing quote, each with a live Held/Stale/
/// Violated verdict. The cheap row (min-wall) updates every drag frame; the
/// expensive rows dim to "recomputing" while dragging and snap crisp on settle.
/// "Make it" unlocks ONLY when every gating check holds — and dies red the
/// instant one violates. No theater: the quote is a real kernel cost, the lead
/// time a labelled estimate.
struct ReceiptLedger: View {
    let model: EditorModel

    private struct Verdict {
        let label: String
        let detail: String
        let held: Bool
        let stale: Bool
    }

    private var verdicts: [Verdict] {
        [
            // Mechanical — LIVE: pure connector_x arithmetic, never stale.
            Verdict(label: "Min wall",
                    detail: String(format: "%.1f mm", max(0, model.connectorMinWall)),
                    held: model.connectorOK, stale: false),
            // Electrical — settles on release.
            Verdict(label: "Copper routed",
                    detail: model.copperUnrouted == 0
                        ? "0 unrouted" : "\(model.copperUnrouted) unrouted",
                    held: model.copperUnrouted == 0, stale: model.receiptStale),
            // Sheet metal — settles on release.
            Verdict(label: "Bracket fold",
                    detail: model.bracketOK
                        ? (model.bracketSeverity == 1 ? "DFM warning" : "foldable")
                        : "not foldable",
                    held: model.bracketOK, stale: model.receiptStale),
        ]
    }

    /// The honest gate: every gating row holds AND nothing is recomputing.
    private var gateOpen: Bool {
        model.allHeld && model.connectorOK && !model.receiptStale
    }
    private var anyViolated: Bool { verdicts.contains { !$0.held && !$0.stale } }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            header
            VStack(spacing: 0) {
                ForEach(Array(verdicts.enumerated()), id: \.offset) { _, v in verdictRow(v) }
            }
            Divider().overlay(.white.opacity(0.08))
            quoteRow
            makeButton
        }
        .padding(14)
        .glassCard()
        .overlay(
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .strokeBorder((anyViolated ? Color.orange : Color.green).opacity(0.32), lineWidth: 1)
        )
        .animation(.snappy(duration: 0.2), value: anyViolated)
        .animation(.snappy(duration: 0.2), value: model.receiptStale)
        .animation(.snappy(duration: 0.2), value: model.connectorOK)
    }

    private var header: some View {
        HStack(spacing: 6) {
            Image(systemName: "checklist").font(.system(size: 11))
            Text("RECEIPT").font(.system(size: 10, weight: .semibold)).tracking(0.6)
            Spacer()
            Text("connector \(Int(model.connectorX.rounded())) mm")
                .font(.system(size: 10, design: .monospaced))
        }
        .foregroundStyle(.tertiary)
    }

    @ViewBuilder private func verdictRow(_ v: Verdict) -> some View {
        HStack(spacing: 8) {
            Image(systemName: v.held ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                .font(.system(size: 12))
                .foregroundStyle(v.held ? Color.green : Color.orange)
                .frame(width: 16)
            Text(v.label).font(.system(size: 12))
            Spacer(minLength: 8)
            if v.stale {
                ProgressView().controlSize(.mini).scaleEffect(0.7)
                Text("recomputing").font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.tertiary)
            } else {
                Text(v.detail).font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(v.held ? .secondary : Color.orange)
            }
        }
        .padding(.vertical, 6)
        .opacity(v.stale ? 0.5 : 1)
    }

    /// Quote — green↔amber only, NEVER red, NEVER gates (sourcing never gates).
    /// The "est." suffix is load-bearing honesty: the bracket cost is a real
    /// kernel model, but the lead time + board/enclosure lines are estimates.
    private var quoteRow: some View {
        HStack(spacing: 8) {
            Image(systemName: "shippingbox.fill").font(.system(size: 12))
                .foregroundStyle(.secondary).frame(width: 16)
            Text("Quote").font(.system(size: 12))
            Spacer()
            if model.receiptStale {
                ProgressView().controlSize(.mini).scaleEffect(0.7)
            } else {
                Text(String(format: "$%.2f · %d day · est.",
                            Double(model.quoteCents) / 100.0, model.leadDays))
                    .font(.system(size: 12, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
        }
    }

    @ViewBuilder private var makeButton: some View {
        Button {
            // The order itself must route through a human-signed authorize_spend
            // gate (deferred) — for now the felt "verified" confirmation.
            model.chime.play(.solved)
        } label: {
            HStack(spacing: 6) {
                Image(systemName: "hammer.fill").font(.system(size: 12))
                Text(model.receiptStale ? "Recomputing…"
                     : (gateOpen ? "Make it" : "Resolve violations first"))
                    .font(.system(size: 13, weight: .medium))
                Spacer()
            }
            .padding(.horizontal, 12).padding(.vertical, 9)
            .frame(maxWidth: .infinity)
            .background(gateOpen ? Color.green.opacity(0.22) : Color.orange.opacity(0.12),
                        in: RoundedRectangle(cornerRadius: 10, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder((gateOpen ? Color.green : Color.orange).opacity(0.5), lineWidth: 1))
            .foregroundStyle(gateOpen ? Color.green : Color.orange)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!gateOpen)
        .help(gateOpen ? "Every cross-domain check holds"
                       : "Make-it unlocks only when every check passes")
    }
}
