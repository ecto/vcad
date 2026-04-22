// Polyfill crypto.randomUUID() for non-secure contexts (HTTP over LAN during
// local development). This file is imported before any other module so the
// polyfill is in place before downstream code reaches for randomUUID.
//
// Moved out of index.html so the deployed app can ship a strict CSP that
// does not need 'unsafe-inline' for script-src.

// Cast through `unknown` so we can both read a possibly-missing method and
// assign our replacement without conflicting with the DOM lib's narrower
// signature (`() => ${string}-...`).
type MutableCrypto = { randomUUID?: () => string; getRandomValues: Crypto["getRandomValues"] };

if (typeof crypto !== "undefined") {
  const c = crypto as unknown as MutableCrypto;
  if (typeof c.randomUUID !== "function") {
    console.warn(
      "[vcad] crypto.randomUUID unavailable (non-secure context). Using polyfill — do NOT use in production.",
    );
    c.randomUUID = function () {
      return "10000000-1000-4000-8000-100000000000".replace(/[018]/g, function (ch) {
        return (
          +ch ^
          (c.getRandomValues(new Uint8Array(1))[0]! & (15 >> (+ch / 4)))
        ).toString(16);
      });
    };
  }
}
