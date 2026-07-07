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
