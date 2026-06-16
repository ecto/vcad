/** A small, dependency-free, synchronous content hash — browser- and node-safe
 *  (no `node:crypto`, no async `crypto.subtle`). Not cryptographic: it only needs
 *  to be deterministic and collision-resistant enough to detect that a board's
 *  DRC result has (not) changed. Four independent 32-bit FNV-1a-style mixers →
 *  128 bits of hex. */
export const HASH_ALGO = "fnv1a-128";

export function hashHex(str: string): string {
  let h1 = 0x811c9dc5,
    h2 = 0x9e3779b9,
    h3 = 0xdeadbeef,
    h4 = 0xcafebabe;
  for (let i = 0; i < str.length; i++) {
    const c = str.charCodeAt(i);
    h1 = Math.imul(h1 ^ c, 0x01000193);
    h2 = Math.imul(h2 ^ c, 0x85ebca6b);
    h3 = Math.imul(h3 ^ c, 0xc2b2ae35);
    h4 = Math.imul(h4 ^ c, 0x27d4eb2f);
  }
  const hx = (n: number) => (n >>> 0).toString(16).padStart(8, "0");
  return hx(h1) + hx(h2) + hx(h3) + hx(h4);
}
