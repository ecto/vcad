/**
 * PCB adapter for the unified DesignReceipt (schema `vcad.receipt/1`).
 *
 * The schema's source of truth is Rust (`crates/vcad-receipt`, ts-rs-generated
 * into `@vcad/ir`); this module mirrors its builder + fail-closed rollup
 * semantics for the TypeScript-side producer, and converts the PCB
 * `build_receipt` output (the re-runnable `Receipt`) into unified claims.
 *
 * House rule (fail-closed, #343): an oracle that could not run yields an
 * `unverifiable` claim — never conflated with a clean pass — and a receipt
 * with zero claims, or any unverifiable claim, never rolls up to `pass`.
 */

import type {
  ClaimQuantity,
  ClaimVerdict,
  DesignReceipt,
  OracleRef,
  Receipt,
  ReceiptClaim,
  ReceiptSummary,
} from "@vcad/ir";
import type { EnclosureFitReport } from "@vcad/engine";

/** Keep in sync with `vcad_receipt::RECEIPT_SCHEMA`. */
export const RECEIPT_SCHEMA = "vcad.receipt/1";

const PCB_DOMAIN = "pcb";

function claim(
  id: string,
  domain: string,
  description: string,
  oracle: OracleRef,
  verdict: ClaimVerdict,
  extra?: Partial<
    Pick<ReceiptClaim, "subject" | "predicted" | "measured" | "details">
  >,
): ReceiptClaim {
  return { id, domain, description, oracle, verdict, ...extra };
}

/** An unverifiable claim; the reason is mandatory, matching the Rust builder. */
export function unverifiableClaim(
  id: string,
  domain: string,
  description: string,
  oracle: OracleRef,
  reason: string,
): ReceiptClaim {
  return claim(id, domain, description, oracle, "unverifiable", {
    details: reason,
  });
}

/**
 * Fail-closed rollup, mirroring `DesignReceipt::overall` in Rust: any fail
 * fails the receipt; otherwise any unverifiable claim — or an empty claim
 * list, which is no evidence, not clean — is unverifiable.
 */
export function overallVerdict(claims: readonly ReceiptClaim[]): ClaimVerdict {
  if (claims.some((c) => c.verdict === "fail")) return "fail";
  if (claims.length === 0 || claims.some((c) => c.verdict === "unverifiable")) {
    return "unverifiable";
  }
  return "pass";
}

/** Counts by verdict plus the rollup, mirroring `DesignReceipt::summary`. */
export function summarize(receipt: DesignReceipt): ReceiptSummary {
  const count = (v: ClaimVerdict) =>
    receipt.claims.filter((c) => c.verdict === v).length;
  return {
    total: receipt.claims.length,
    passed: count("pass"),
    failed: count("fail"),
    unverifiable: count("unverifiable"),
    overall: overallVerdict(receipt.claims),
  };
}

/** `"vcad-ecad-pcb 0.9.4"` → `{ id, version }`; no version reads as unknown. */
function oracleFromBackend(backend: string): OracleRef {
  const at = backend.lastIndexOf(" ");
  if (at === -1) return { id: backend || "unknown", version: "unknown" };
  return { id: backend.slice(0, at), version: backend.slice(at + 1) };
}

function quantity(value: number | string | boolean, unit?: string): ClaimQuantity {
  return unit === undefined ? { value } : { value, unit };
}

/** Cross-domain extras that ride next to the PCB receipt. */
export interface PcbReceiptExtras {
  /** Cross-domain enclosure-fit verdict, when an enclosure was supplied. */
  enclosureFit?: EnclosureFitReport;
  /** Why the enclosure-fit oracle could not run (fail-closed: becomes an
   *  unverifiable claim, never a silent omission). */
  enclosureFitError?: string;
}

/** The enclosure-fit report carries no oracle version — say so honestly. */
const ENCLOSURE_ORACLE: OracleRef = {
  id: "vcad-engine/enclosure-fit",
  version: "unknown",
};

/**
 * Convert a PCB `Receipt` (build_receipt output) into a unified
 * DesignReceipt. The board hash becomes the document fingerprint; the DRC
 * summary becomes a board-level cleanliness claim plus one claim per
 * violated rule; power-plane continuity, part provenance, and the sourcing
 * snapshot follow (sourcing never gates an electrical verdict). Enclosure-fit
 * extras become cross-domain claims — a fit oracle that couldn't run is an
 * unverifiable claim, not a missing one.
 */
export function unifiedFromPcbReceipt(
  receipt: Receipt,
  documentId?: string,
  extras?: PcbReceiptExtras,
): DesignReceipt {
  const oracle = oracleFromBackend(receipt.drc_backend);
  const claims: ReceiptClaim[] = [];

  const total = receipt.drc.total;
  claims.push(
    claim(
      "pcb.drc.clean",
      PCB_DOMAIN,
      "board passes design-rule check",
      oracle,
      total === 0 ? "pass" : "fail",
      {
        predicted: quantity(0, "violations"),
        measured: quantity(total, "violations"),
        details: `design rules hash ${receipt.design_rules_hash}`,
      },
    ),
  );
  for (const { rule, count } of receipt.drc.by_rule) {
    claims.push(
      claim(
        `pcb.drc.${rule}`,
        PCB_DOMAIN,
        `no ${rule} violations`,
        oracle,
        count === 0 ? "pass" : "fail",
        {
          predicted: quantity(0, "violations"),
          measured: quantity(count, "violations"),
        },
      ),
    );
  }

  // Realized-copper continuity per power/plane net: a split plane is an open
  // PDN even when clearance/short DRC is clean, so it gets its own claim and
  // can fail the receipt independently of the DRC claims.
  for (const p of receipt.power_integrity ?? []) {
    claims.push(
      claim(
        "pcb.power.continuity",
        PCB_DOMAIN,
        `net '${p.net}' copper forms one continuous island`,
        oracle,
        p.continuous ? "pass" : "fail",
        {
          subject: `net:${p.net}`,
          predicted: quantity(1, "islands"),
          measured: quantity(p.islands, "islands"),
          details: `${p.connected_pads}/${p.total_pads} pads on main island (${Math.round(p.coverage * 1000) / 10}%), ${p.vias} stitching via(s)`,
        },
      ),
    );
  }

  const missingMpn = receipt.parts.filter((p) => !p.mpn).length;
  claims.push(
    claim(
      "pcb.provenance.parts",
      PCB_DOMAIN,
      "every placed part is recorded with footprint and value",
      oracle,
      "pass",
      {
        measured: quantity(receipt.parts.length, "parts"),
        ...(missingMpn > 0
          ? { details: `${missingMpn} part(s) without an MPN` }
          : {}),
      },
    ),
  );

  if (receipt.sourcing) {
    claims.push(
      claim(
        "pcb.sourcing.snapshot",
        PCB_DOMAIN,
        "sourcing snapshot captured at receipt time",
        oracle,
        "pass",
        {
          measured: quantity(receipt.sourcing.lines.length, "lines"),
          details: "informational — sourcing drift never gates the DRC verdict",
        },
      ),
    );
  }

  if (extras?.enclosureFitError) {
    claims.push(
      unverifiableClaim(
        "pcb.enclosure_fit",
        PCB_DOMAIN,
        "board fits its enclosure",
        ENCLOSURE_ORACLE,
        extras.enclosureFitError,
      ),
    );
  } else if (extras?.enclosureFit) {
    for (const check of extras.enclosureFit.checks) {
      // pass|warn hold (warn keeps its caveat in details); fail fails;
      // skip means the check did not run — unverifiable, never clean.
      const verdict: ClaimVerdict =
        check.status === "fail"
          ? "fail"
          : check.status === "skip"
            ? "unverifiable"
            : "pass";
      claims.push(
        claim(
          `pcb.enclosure_fit.${check.id}`,
          PCB_DOMAIN,
          check.label,
          ENCLOSURE_ORACLE,
          verdict,
          {
            details:
              check.status === "warn" ? `warning: ${check.detail}` : check.detail,
          },
        ),
      );
    }
  }

  return {
    schema: RECEIPT_SCHEMA,
    ...(documentId ? { document_id: documentId } : {}),
    document_fingerprint: receipt.board_hash,
    claims,
  };
}

/**
 * Fail-closed stand-in for when the PCB oracle could not run at all
 * (no board in the document, or the ECAD engine is unavailable): a receipt
 * whose single claim is unverifiable, so it can never read as clean.
 */
export function unverifiablePcbReceipt(
  reason: string,
  documentId?: string,
): DesignReceipt {
  return {
    schema: RECEIPT_SCHEMA,
    ...(documentId ? { document_id: documentId } : {}),
    claims: [
      unverifiableClaim(
        "pcb.drc.clean",
        PCB_DOMAIN,
        "board passes design-rule check",
        { id: "vcad-ecad-pcb", version: "unknown" },
        reason,
      ),
    ],
  };
}

// ─── Spec-first mechanical verification (verify_spec) ─────────────────────────
//
// "TDD for CAD": the caller declares a spec (bbox, volume range, watertight,
// part count, center of mass) BEFORE the geometry exists, then iterates the
// document until every claim rolls up to pass. The measurement comes from the
// kernel tessellation (computeIntegrity); the same fail-closed semantics as
// the PCB adapter apply — an empty spec, a missing measurement, or a claim the
// kernel can't evaluate is `unverifiable`, never a silent pass.

const MECH_DOMAIN = "mechanical";

/** The oracle behind spec claims: geometry measured from the kernel
 *  tessellation. The engine exposes no version, so it reads as unknown —
 *  honest, and (unlike a fabricated version) never masquerades as a pass. */
const SPEC_ORACLE: OracleRef = { id: "vcad-kernel/integrity", version: "unknown" };

/** Default ± tolerance (mm) for bbox / center-of-mass axes when the caller
 *  declares none. */
export const DEFAULT_SPEC_TOL_MM = 0.01;

interface XYZ {
  x: number;
  y: number;
  z: number;
}

/** An axis-bounded point target: any subset of x/y/z, plus an optional ± tol
 *  (mm) applied per declared axis. A left-out axis produces no claim. */
export interface PointSpec {
  x?: number;
  y?: number;
  z?: number;
  tol?: number;
}

/**
 * Caller-supplied spec — the "declare first, iterate to green" contract for
 * verify_spec. Every field is optional; a field left out simply produces no
 * claim. A spec that declares nothing is unverifiable (no evidence), never a
 * pass.
 */
export interface DesignSpec {
  /** Bounding-box minimum corner (any subset of axes) ± tol. */
  bbox_min?: PointSpec;
  /** Bounding-box maximum corner (any subset of axes) ± tol. */
  bbox_max?: PointSpec;
  /** Enclosed volume must fall within [min, max] mm³ (either bound optional). */
  volume?: { min?: number; max?: number };
  /** Whether the solid must be a closed, watertight manifold. */
  watertight?: boolean;
  /** Exact number of parts the document must contain. */
  part_count?: number;
  /** Center of mass (any subset of axes) ± tol. */
  center_of_mass?: PointSpec;
}

/** The measured geometry a spec is graded against — the subset of the kernel
 *  integrity report the claims read. `null` bbox / CoM mean the kernel could
 *  not produce that measurement (open or empty geometry). */
export interface SpecMeasurement {
  volume_mm3: number;
  bounding_box: { min: XYZ; max: XYZ } | null;
  center_of_mass: XYZ | null;
  watertight: boolean;
  parts: number;
}

const round3 = (v: number): number => Math.round(v * 1000) / 1000;

/** Per-axis claims for a point target (a bbox corner or the CoM). When the
 *  measurement is null the kernel could not produce it, so every declared axis
 *  is unverifiable — never a silent pass. */
function pointClaims(
  idPrefix: string,
  label: string,
  target: PointSpec,
  measured: XYZ | null,
  reasonIfMissing: string,
): ReceiptClaim[] {
  const tol = target.tol ?? DEFAULT_SPEC_TOL_MM;
  const out: ReceiptClaim[] = [];
  for (const axis of ["x", "y", "z"] as const) {
    const want = target[axis];
    if (want === undefined) continue;
    const id = `${idPrefix}.${axis}`;
    const description = `${label} ${axis} within ±${tol} mm of ${want}`;
    if (measured === null) {
      out.push(
        unverifiableClaim(id, MECH_DOMAIN, description, SPEC_ORACLE, reasonIfMissing),
      );
      continue;
    }
    const got = measured[axis];
    out.push(
      claim(id, MECH_DOMAIN, description, SPEC_ORACLE, Math.abs(got - want) <= tol ? "pass" : "fail", {
        predicted: quantity(want, "mm"),
        measured: quantity(round3(got), "mm"),
        details: `tolerance ±${tol} mm`,
      }),
    );
  }
  return out;
}

/** Volume-range claim. An unbounded range (no min and no max) asserts nothing
 *  measurable and is unverifiable, not a vacuous pass. */
function volumeClaim(range: { min?: number; max?: number }, v: number): ReceiptClaim {
  const { min: lo, max: hi } = range;
  if (lo === undefined && hi === undefined) {
    return unverifiableClaim(
      "spec.volume",
      MECH_DOMAIN,
      "volume within declared range",
      SPEC_ORACLE,
      "volume spec declared neither a min nor a max bound",
    );
  }
  const rangeLabel =
    lo !== undefined && hi !== undefined ? `[${lo}, ${hi}]` : lo !== undefined ? `≥ ${lo}` : `≤ ${hi}`;
  const okLo = lo === undefined || v >= lo;
  const okHi = hi === undefined || v <= hi;
  return claim("spec.volume", MECH_DOMAIN, `volume within ${rangeLabel} mm³`, SPEC_ORACLE, okLo && okHi ? "pass" : "fail", {
    predicted: quantity(rangeLabel, "mm^3"),
    measured: quantity(round3(v), "mm^3"),
  });
}

/**
 * Grade a caller-supplied `spec` against the kernel `measurement` and roll it
 * up into a fail-closed DesignReceipt (one claim per declared field, each
 * carrying measured-vs-expected). Pass `measurement: null` when the kernel
 * could not evaluate the document at all — every declared claim becomes
 * unverifiable, so the receipt can never read as clean.
 */
export function unifiedFromSpec(
  spec: DesignSpec,
  measurement: SpecMeasurement | null,
  documentId?: string,
  fingerprint?: string,
): DesignReceipt {
  const claims: ReceiptClaim[] = [];
  const evalFailed = "kernel could not evaluate the document — no geometry to measure";

  if (spec.bbox_min) {
    claims.push(
      ...pointClaims(
        "spec.bbox.min",
        "bounding-box min",
        spec.bbox_min,
        measurement?.bounding_box?.min ?? null,
        measurement === null ? evalFailed : "document has no bounding box (no evaluable geometry)",
      ),
    );
  }
  if (spec.bbox_max) {
    claims.push(
      ...pointClaims(
        "spec.bbox.max",
        "bounding-box max",
        spec.bbox_max,
        measurement?.bounding_box?.max ?? null,
        measurement === null ? evalFailed : "document has no bounding box (no evaluable geometry)",
      ),
    );
  }
  if (spec.volume) {
    claims.push(
      measurement === null
        ? unverifiableClaim("spec.volume", MECH_DOMAIN, "volume within declared range", SPEC_ORACLE, evalFailed)
        : volumeClaim(spec.volume, measurement.volume_mm3),
    );
  }
  if (spec.watertight !== undefined) {
    const want = spec.watertight;
    claims.push(
      measurement === null
        ? unverifiableClaim("spec.watertight", MECH_DOMAIN, "solid watertightness", SPEC_ORACLE, evalFailed)
        : claim(
            "spec.watertight",
            MECH_DOMAIN,
            want ? "solid is watertight (closed manifold)" : "solid is not watertight",
            SPEC_ORACLE,
            measurement.watertight === want ? "pass" : "fail",
            { predicted: quantity(want), measured: quantity(measurement.watertight) },
          ),
    );
  }
  if (spec.part_count !== undefined) {
    const want = spec.part_count;
    claims.push(
      measurement === null
        ? unverifiableClaim("spec.part_count", MECH_DOMAIN, `document has ${want} part(s)`, SPEC_ORACLE, evalFailed)
        : claim("spec.part_count", MECH_DOMAIN, `document has ${want} part(s)`, SPEC_ORACLE, measurement.parts === want ? "pass" : "fail", {
            predicted: quantity(want, "count"),
            measured: quantity(measurement.parts, "count"),
          }),
    );
  }
  if (spec.center_of_mass) {
    claims.push(
      ...pointClaims(
        "spec.com",
        "center of mass",
        spec.center_of_mass,
        measurement?.center_of_mass ?? null,
        measurement === null ? evalFailed : "center of mass is undefined (no enclosed volume)",
      ),
    );
  }

  // Fail-closed: a spec that declares nothing measurable is not a pass. An
  // empty claim list already rolls up to unverifiable (overallVerdict), but
  // carry an explicit claim so the receipt says WHY it can't be clean.
  if (claims.length === 0) {
    claims.push(
      unverifiableClaim(
        "spec.empty",
        MECH_DOMAIN,
        "spec declares at least one claim",
        SPEC_ORACLE,
        "spec is empty — no claims to verify",
      ),
    );
  }

  return {
    schema: RECEIPT_SCHEMA,
    ...(documentId ? { document_id: documentId } : {}),
    ...(fingerprint ? { document_fingerprint: fingerprint } : {}),
    claims,
  };
}
