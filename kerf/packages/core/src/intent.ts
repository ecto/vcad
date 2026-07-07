/**
 * OrderIntent — the typed artifact the whole system executes.
 *
 * The Tier-2 agent never carries a prose goal; it carries one of these.
 * Three shapes cover every commerce surface a hardware project touches:
 * configurators (SendCutSend, JLCPCB), catalogs (McMaster, DigiKey), and
 * RFQ-by-email (the job-shop long tail).
 *
 * Money is integer MINOR units (cents) everywhere — never floats.
 */

/** An exact amount in a currency's minor units. */
export interface Money {
  currency: string;
  amount_minor: number;
}

/** A file the job will upload, pinned by content hash. The runtime hashes
 *  the exact bytes it feeds the vendor's file input; matching this value is
 *  the `kerf/upload-hash` oracle. */
export interface FileRef {
  name: string;
  bytes: number;
  sha256: string;
  media_type?: string;
}

/** Destination address. */
export interface ShipTo {
  name: string;
  line1: string;
  line2?: string;
  city: string;
  region: string;
  postal_code: string;
  country: string;
}

interface IntentBase {
  /** Unique per order attempt. Written into the vendor's PO/notes field
   *  when the vendor supports one, so reconciliation can find the order
   *  server-side. Never reused after a `PLACING` attempt. */
  idempotency_key: string;
  /** Registry vendor id, e.g. `"sendcutsend"`. */
  vendor: string;
  ship_to: ShipTo;
  /** Hard ceiling. Must equal the upstream mandate cap; the virtual card
   *  is funded to exactly this (+ shipping tolerance). */
  budget_cap: Money;
  /** ISO-8601. Advisory for shipping-speed selection. */
  deadline?: string;
}

/** Custom-manufactured part on an instant-quote configurator. `config`
 *  keys/values are VENDOR-NATIVE labels as declared by the vendor's
 *  registry `config_schema` — the playbook selects options by label. */
export interface ConfiguratorIntent extends IntentBase {
  kind: "configurator";
  files: FileRef[];
  /** Process key, e.g. `"sheet_metal"` — must be in the vendor manifest. */
  process: string;
  config: Record<string, string | number | boolean>;
  quantity: number;
}

/** Fixed-SKU purchase from a catalog vendor. */
export interface CatalogIntent extends IntentBase {
  kind: "catalog";
  items: Array<{ sku: string; quantity: number; description?: string }>;
}

/** Request-for-quote by email or form: files plus a human-readable spec.
 *  Same state machine, slow-motion — the vendor's reply is the quote. */
export interface RfqIntent extends IntentBase {
  kind: "rfq";
  files: FileRef[];
  spec_text: string;
  quantity: number;
}

/** The tagged union every kerf job executes. */
export type OrderIntent = ConfiguratorIntent | CatalogIntent | RfqIntent;
