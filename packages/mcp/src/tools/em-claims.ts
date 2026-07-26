/**
 * The electromagnetics (EM) receipt-claim family.
 *
 * Every EM calculator (calc_coil, calc_motor, calc_rf, calc_impedance,
 * size_coil, size_impedance, size_pdn, winding_layout, check_self_start)
 * predicts values for named physical quantities — inductance, torque
 * constant, Z0, winding factor, resonance, locked-rotor torque, starting
 * margin. A claim records one such prediction in a uniform,
 * plainly serializable shape: what quantity, what value, by what method,
 * from what inputs. That is exactly what a receipt needs to ledger the
 * prediction and what a later measurement or FEA pass needs to grade it.
 *
 * Deliberately dependency-free: coordination with the unified-receipt work
 * is by shape, not by import. Claims ride inside each tool's existing JSON
 * payload under a `claims` key — additive, so no consumer breaks.
 *
 * Method identifiers are stable, citable strings (see `EmMethod`); a claim
 * is honest about being a first-order closed-form model, so a higher-
 * fidelity oracle can grade it rather than contradict it.
 */

/** Physical quantities the EM domain can currently claim. */
export type EmQuantity =
  | "inductance"
  | "dc_resistance"
  | "characteristic_impedance"
  | "differential_impedance"
  | "effective_permittivity"
  | "propagation_delay"
  | "ir_drop"
  | "resonant_frequency"
  | "q_factor"
  | "winding_factor"
  | "torque_constant"
  | "back_emf_constant"
  | "no_load_speed"
  | "stall_torque"
  | "airgap_flux_density"
  | "tooth_flux_density"
  | "torque_per_unit_slip"
  | "locked_rotor_torque"
  | "synchronous_speed"
  | "rotor_copper_loss"
  | "friction_torque"
  | "start_margin";

/** Stable identifiers for the closed-form models behind each claim. */
export type EmMethod =
  | "wheeler-mohan-1999" // modified-Wheeler planar spiral inductance
  | "dc-trace-resistance" // R = ρ·L/(w·t)
  | "ipc2141-microstrip" // Hammerstad–Jensen microstrip Z0 / εr_eff
  | "ipc2141-stripline" // IPC-2141 stripline Z0
  | "edge-coupled-diff-pair" // Zdiff = 2·Z0·k(s/h)
  | "dc-resistor-mesh" // PDN Laplacian G·V = I
  | "rlc-analytic" // series/parallel RLC frequency response
  | "star-of-slots" // polyphase winding factor kw = kp·kd
  | "mec-reluctance" // air-gap B via magnetic equivalent circuit
  | "first-order-dc-motor" // Kt/Ke, V = iR + Ke·ω envelope
  | "mec-fringing-derate" // Carter-like w/(w+2g) pole-edge fringing on the MEC B
  | "mec-tooth-concentration" // tooth B from gap B x (pitch/width), LINEAR iron
  | "mec-saturating-iron" // MEC solved with the arctangent B-H law (iron_js_t)
  | "thin-sheet-induction" // rotating-MMF B1 + linear eddy slip torque (Russell–Norsworthy end effect)
  | "bearing-friction-catalog" // documented typical bearing running-drag ranges
  | "torque-friction-margin"; // starting torque vs worst-case friction, fail-closed

/**
 * One quantitative prediction made by an EM calculator — the unit of the
 * domain's receipt-claim family.
 */
export interface EmClaim {
  /** Domain tag, so a mixed-domain receipt can group claims. */
  domain: "em";
  quantity: EmQuantity;
  /** Predicted value, in `unit`. */
  predicted: number;
  /** Unit of `predicted` as the tool reports it (e.g. "nH", "ohm", "rad/s"). */
  unit: string;
  /** The model that produced the number. */
  method: EmMethod;
  /** Named inputs the prediction was computed from. */
  inputs: Record<string, number | string>;
}

/** Build one claim. Skips nothing, validates nothing — callers own honesty. */
export function emClaim(
  quantity: EmQuantity,
  predicted: number,
  unit: string,
  method: EmMethod,
  inputs: Record<string, number | string>,
): EmClaim {
  return { domain: "em", quantity, predicted, unit, method, inputs };
}
