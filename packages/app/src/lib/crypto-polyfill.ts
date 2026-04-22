// Polyfill crypto.randomUUID() for non-secure contexts (HTTP over LAN during
// local development). This file is imported before any other module so the
// polyfill is in place before downstream code reaches for randomUUID.
//
// Moved out of index.html so the deployed app can ship a strict CSP that
// does not need 'unsafe-inline' for script-src.

if (
  typeof crypto !== "undefined" &&
  typeof (crypto as Crypto & { randomUUID?: unknown }).randomUUID !== "function"
) {
  console.warn(
    "[vcad] crypto.randomUUID unavailable (non-secure context). Using polyfill — do NOT use in production.",
  );
  (crypto as Crypto & { randomUUID: () => string }).randomUUID = function () {
    return "10000000-1000-4000-8000-100000000000".replace(/[018]/g, function (c) {
      return (
        +c ^
        (crypto.getRandomValues(new Uint8Array(1))[0]! & (15 >> (+c / 4)))
      ).toString(16);
    });
  };
}
