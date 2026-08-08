/**
 * The influence surface in the Parameters panel: which knob moves a quantity,
 * how much of it that knob commands, and how far the answer may be acted on.
 *
 * The two behaviours worth pinning are the ones a reader will act on:
 *  - the panel **reorders itself** by influence, so the knob that matters is
 *    at the top rather than wherever the alphabet put it;
 *  - a row whose derivative could not be established renders as *unverifiable*
 *    rather than as a confident number. A gradient with a missing coupling
 *    term can carry the wrong sign, and the UI must not launder that into a
 *    tidy figure next to a scrub input.
 */
import { describe, it, expect, afterEach, beforeEach } from "vitest";
import { render, cleanup, screen } from "@testing-library/react";
import type { SensitivityReport, SensitivityRow } from "@vcad/engine";
import { ParametersPanel } from "@/components/ParametersPanel";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  mergeParametersIntoDocument,
  useDocumentStore,
  useParametersStore,
} from "@vcad/core";
import {
  documentRevision,
  formatDerivative,
  influenceOf,
  rankedRows,
  trustLabel,
  useSensitivityStore,
} from "@/stores/sensitivity-store";

function row(
  parameter: string,
  value: number,
  trust: [number, number] | null,
  extra: Partial<SensitivityRow> = {},
): SensitivityRow {
  return {
    parameter,
    objective: "mass",
    value,
    unit: "g/mm",
    at: trust ? (trust[0] + trust[1]) / 2 : 1,
    route: { route: "dual" },
    basis: "verified",
    verdict: "pass",
    ...(trust
      ? { trust: { lower: trust[0], upper: trust[1], limited_by: "topology_stable" as const } }
      : {}),
    ...extra,
  };
}

function report(rows: SensitivityRow[]): SensitivityReport {
  return {
    table: { rows },
    rendered: "",
    ranked: {},
    unusable: rows
      .filter((r) => r.verdict === "unverifiable")
      .map((r) => `${r.objective}/${r.parameter}`),
    allUsable: rows.every((r) => r.verdict !== "unverifiable"),
    claims: [],
  };
}

/**
 * The revision the panel computes: parameters and bindings live in their own
 * store, so the panel differentiates the *merged* document. Mirroring that
 * here is what keeps a seeded report from being invalidated on first render.
 */
function panelRevision(): string {
  const { parameters, bindings } = useParametersStore.getState();
  return documentRevision(
    mergeParametersIntoDocument(
      useDocumentStore.getState().document,
      parameters,
      bindings,
    ),
  );
}

describe("influence helpers", () => {
  it("ranks by command over the quantity, not by raw slope", () => {
    // A steep knob you can barely move commands less than a gentle one with
    // room to travel: 100 x 0.02 = 2, versus 1 x 10 = 10.
    const steep = row("fin_thickness", 100, [0.99, 1.01]);
    const gentle = row("wall", 1, [0, 10]);
    expect(influenceOf(steep)).toBeCloseTo(2, 9);
    expect(influenceOf(gentle)).toBeCloseTo(10, 9);

    const { rows, max } = rankedRows(report([steep, gentle]), "mass");
    expect(rows.map((r) => r.parameter)).toEqual(["wall", "fin_thickness"]);
    expect(max).toBeCloseTo(10, 9);
  });

  it("sorts rows with no trust radius last — they are not comparable", () => {
    const unbounded = row("mystery", 1e9, null);
    const bounded = row("wall", 1, [0, 10]);
    expect(influenceOf(unbounded)).toBeNull();
    const { rows } = rankedRows(report([unbounded, bounded]), "mass");
    expect(rows.map((r) => r.parameter)).toEqual(["wall", "mystery"]);
  });

  it("labels the trust radius with why it ends", () => {
    expect(trustLabel(row("wall", 1, [1.25, 3.5]))).toBe("valid 1.25–3.50 (topology)");
    // One precision across the pair, driven by the larger end.
    expect(trustLabel(row("wall", 1, [0, 10]))).toBe("valid 0.00–10.00 (topology)");
    expect(trustLabel(row("wall", 1, null))).toBeNull();
  });

  it("formats derivatives readably across magnitudes", () => {
    expect(formatDerivative(row("a", 2.5, [0, 1]))).toBe("2.50 g/mm");
    expect(formatDerivative(row("a", 143.2, [0, 1]))).toBe("143 g/mm");
    expect(formatDerivative(row("a", 1.2e-6, [0, 1]))).toBe("1.20e-6 g/mm");
    expect(formatDerivative(row("a", 4.4e7, [0, 1]))).toBe("4.40e+7 g/mm");
    expect(formatDerivative(row("a", Number.NaN, [0, 1]))).toBe("—");
  });
});

describe("ParametersPanel influence rows", () => {
  // `cleanup` belongs in afterEach. Run only in beforeEach it leaves the last
  // test's panel mounted, and that panel's invalidate() effect then wipes the
  // store out from under whatever describe runs next.
  afterEach(cleanup);

  beforeEach(() => {
    useParametersStore.setState({
      parameters: {
        // Deliberately alphabetical-last for the dominant knob, so a passing
        // order test proves the ranking drove it.
        wall: { value: 2 },
        fin_thickness: { value: 1 },
      },
      bindings: {},
    } as never);
    useSensitivityStore.setState({
      report: null,
      loading: false,
      error: null,
      quantity: "mass",
      computedFor: null,
    });
  });

  it("shows the definitions with no influence data until asked", () => {
    render(
      <TooltipProvider>
        <ParametersPanel />
      </TooltipProvider>,
    );
    expect(screen.getByText(/Influence/)).toBeTruthy();
    // The prompt, not numbers.
    expect(screen.getByText(/one\s+gradient pass/)).toBeTruthy();
  });

  it("reorders the panel by influence and shows each derivative", () => {
    useSensitivityStore.setState({
      report: report([
        row("fin_thickness", 100, [0.99, 1.01]),
        row("wall", 1, [0, 10]),
      ]),
      computedFor: panelRevision(),
    });
    const { container } = render(
      <TooltipProvider>
        <ParametersPanel />
      </TooltipProvider>,
    );

    const names = Array.from(
      container.querySelectorAll<HTMLInputElement>('input[aria-label="Parameter name"]'),
    ).map((i) => i.value);
    expect(names).toEqual(["wall", "fin_thickness"]);

    expect(container.textContent).toContain("1.00 g/mm");
    expect(container.textContent).toContain("100 g/mm");
    expect(container.textContent).toContain("valid 0.00–10.00 (topology)");
    // Rank badges, most influential first.
    expect(container.textContent).toContain("#1");
    expect(container.textContent).toContain("#2");
  });

  it("renders an unverifiable row as unverifiable, not as a number to trust", () => {
    useSensitivityStore.setState({
      report: report([
        row("wall", -8.1, [1, 4], {
          verdict: "unverifiable",
          basis: "predicted",
          note: "dflow/dthermal missing",
        }),
        row("fin_thickness", 1, [0, 1]),
      ]),
      computedFor: panelRevision(),
    });
    const { container } = render(
      <TooltipProvider>
        <ParametersPanel />
      </TooltipProvider>,
    );
    expect(container.textContent).toContain("unverifiable");
    expect(container.textContent).toContain("dflow/dthermal missing");
    // And the panel says so at the top, where a decision gets made.
    expect(container.textContent).toContain("must not steer");
  });

  it("marks a finite-difference row as such", () => {
    useSensitivityStore.setState({
      report: report([
        row("wall", 1, [0, 10], { route: { route: "finite_difference", step: 1e-3 } }),
      ]),
      computedFor: panelRevision(),
    });
    const { container } = render(
      <TooltipProvider>
        <ParametersPanel />
      </TooltipProvider>,
    );
    expect(container.textContent).toContain("finite difference");
  });
});

describe("compute supersession", () => {
  const engineFor = (label: string, calls: string[]) =>
    ({
      documentSensitivities: () => {
        calls.push(label);
        return report([row(label, 1, [0, 10])]);
      },
    }) as never;

  beforeEach(() => {
    useSensitivityStore.setState({
      report: null,
      loading: false,
      error: null,
      quantity: "mass",
      computedFor: null,
    });
  });

  it("lets the newest request win instead of dropping it", async () => {
    const calls: string[] = [];
    const doc = useDocumentStore.getState().document;
    const store = useSensitivityStore.getState();

    // Two computes issued back to back — the second arrives while the first
    // is still yielding. Before, the second was silently dropped and the user
    // was left with a result for the document they had a moment ago.
    const first = store.compute(doc, engineFor("first", calls), "rev-a");
    const second = store.compute(doc, engineFor("second", calls), "rev-b");
    await Promise.all([first, second]);

    const state = useSensitivityStore.getState();
    expect(state.computedFor).toBe("rev-b");
    expect(state.report?.table.rows[0]?.parameter).toBe("second");
    expect(state.loading).toBe(false);
  });

  it("does not let a superseded request write a stale report", async () => {
    const calls: string[] = [];
    const doc = useDocumentStore.getState().document;
    const store = useSensitivityStore.getState();

    const stale = store.compute(doc, engineFor("stale", calls), "rev-old");
    const fresh = store.compute(doc, engineFor("fresh", calls), "rev-new");
    await Promise.all([stale, fresh]);

    // The superseded pass may or may not have run, but it must never be the
    // one whose result is in the store.
    expect(useSensitivityStore.getState().report?.table.rows[0]?.parameter).toBe(
      "fresh",
    );
    expect(useSensitivityStore.getState().computedFor).toBe("rev-new");
  });

  it("a failing solve still clears loading and reports the error", async () => {
    const doc = useDocumentStore.getState().document;
    const boom = {
      documentSensitivities: () => {
        throw new Error("kernel exploded");
      },
    } as never;
    await useSensitivityStore.getState().compute(doc, boom, "rev");
    const state = useSensitivityStore.getState();
    expect(state.loading).toBe(false);
    expect(state.error).toContain("kernel exploded");
    expect(state.report).toBeNull();
  });
});
