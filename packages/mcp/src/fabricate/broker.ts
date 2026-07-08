/**
 * Fulfillment Broker — vendor-neutral routing core.
 *
 * Takes a normalized QuoteRequest, fans out to every adapter that serves the
 * process, applies the hidden cost-plus margin + landed cost, normalizes to
 * margin-INCLUSIVE FabOptions, and picks the recommended option (cheapest
 * in-spec, preferring orderable). The per-fab cost and margin never leave this
 * module in the agent-facing options — only in the server-only economics it
 * returns alongside.
 *
 * Phase 0 is quote-only: adapters return estimates, so `orderable` is false for
 * every option (you can quote, not yet order). place_order + binding quotes
 * land in Phase 1.
 */

import { jlcpcbAdapter } from "./adapters/jlcpcb.js";
import { digitalMetalAdapter } from "./adapters/digitalmetal.js";
import { genericEstimateAdapter } from "./adapters/generic.js";
import { kerfAdapter } from "./adapters/kerf.js";
import { applyMargin, estimateLandedCost, marginOf } from "./pricing.js";
import type {
  FabOption,
  LandedCost,
  ManufacturerAdapter,
  QuoteRequest,
} from "./types.js";

/** Fabs with a real (eventual) contract — eligible to be orderable. The
 *  generic estimator never is. Still requires a BINDING quote to flip
 *  orderable true, which no adapter produces yet: the kerf rail's
 *  "sendcutsend" quotes at basis "quoted" (the fab's own displayed price —
 *  it suppresses the generic estimator for sheet metal when it actually
 *  quoted, but stays non-orderable until kerf order execution, Wave 1/2). */
const CONTRACTED_FABS = new Set(["jlcpcb", "digitalmetal", "sendcutsend"]);

export const DEFAULT_ADAPTERS: readonly ManufacturerAdapter[] = [
  jlcpcbAdapter,
  digitalMetalAdapter,
  kerfAdapter,
  genericEstimateAdapter,
];

export interface BrokerResult {
  options: FabOption[];
  /** Recommended option (first after sort), or null if nothing quoted. */
  recommended: FabOption | null;
  landed_cost: LandedCost;
  /** Server-only economics for the recommended option. */
  fab_cost_minor: number;
  margin_minor: number;
}

export class FulfillmentBroker {
  constructor(
    private adapters: readonly ManufacturerAdapter[] = DEFAULT_ADAPTERS,
  ) {}

  async quote(req: QuoteRequest): Promise<BrokerResult> {
    const eligible = this.adapters.filter((a) =>
      a.processes.includes(req.process),
    );

    const built: Array<{ option: FabOption; fabCost: number }> = [];
    for (const adapter of eligible) {
      let q;
      try {
        q = await adapter.quote(req);
      } catch {
        q = null; // a failing adapter drops out, never breaks the quote.
      }
      if (!q) continue;

      const landed = estimateLandedCost({
        region: adapter.region,
        supportsDdp: adapter.supportsDdp,
      });
      const markedUp = applyMargin(q.fab_cost_minor);
      const totalMinor = markedUp + landed.shipping_minor + landed.duty_minor;
      const orderable =
        CONTRACTED_FABS.has(adapter.key) && q.pricing_basis === "binding";

      built.push({
        fabCost: q.fab_cost_minor,
        option: {
          fab: adapter.key,
          fab_label: adapter.label,
          region: adapter.region,
          unit_price_minor: Math.round(totalMinor / req.quantity),
          total_minor: totalMinor,
          lead_time_days: q.lead_time_days,
          in_spec: q.in_spec,
          pricing_basis: q.pricing_basis,
          supports_ddp: adapter.supportsDdp,
          orderable,
          notes: q.notes,
        },
      });
    }

    // The generic estimator is a FALLBACK, not a competitor: drop it whenever a
    // contracted fab already quoted this process, so a non-orderable ballpark
    // can't out-rank or duplicate a real fab (e.g. vcad_estimate undercutting
    // JLCPCB on a 4-layer board). It only stands alone for processes with no
    // contracted fab yet (CNC, sheet metal).
    const hasContracted = built.some((b) => CONTRACTED_FABS.has(b.option.fab));
    const pool = hasContracted
      ? built.filter((b) => CONTRACTED_FABS.has(b.option.fab))
      : built;

    // Sort: in-spec first, then orderable, then cheapest total.
    pool.sort((a, b) => {
      if (a.option.in_spec !== b.option.in_spec) return a.option.in_spec ? -1 : 1;
      if (a.option.orderable !== b.option.orderable)
        return a.option.orderable ? -1 : 1;
      return a.option.total_minor - b.option.total_minor;
    });

    const top = pool[0] ?? null;
    const landed = top
      ? estimateLandedCost({
          region: this.adapterRegion(top.option.fab),
          supportsDdp: top.option.supports_ddp,
        })
      : { shipping_minor: 0, duty_minor: 0, basis: "none" };

    return {
      options: pool.map((b) => b.option),
      recommended: top ? top.option : null,
      landed_cost: landed,
      fab_cost_minor: top ? top.fabCost : 0,
      margin_minor: top ? marginOf(top.fabCost) : 0,
    };
  }

  private adapterRegion(key: string): string {
    return this.adapters.find((a) => a.key === key)?.region ?? "n/a";
  }
}
