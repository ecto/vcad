/**
 * Minimal ambient typings for the `gifenc` package, which ships without
 * its own type declarations. We only describe the surface used by
 * `tools/record.ts`; full API coverage isn't worth the maintenance.
 */
declare module "gifenc" {
  export function GIFEncoder(): {
    writeFrame: (
      indexed: Uint8Array,
      width: number,
      height: number,
      opts: { palette: number[][]; delay: number },
    ) => void;
    finish: () => void;
    bytes: () => Uint8Array;
  };
  export function quantize(rgba: Uint8Array, maxColors: number): number[][];
  export function applyPalette(rgba: Uint8Array, palette: number[][]): Uint8Array;
}
