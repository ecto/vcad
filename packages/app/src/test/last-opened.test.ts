import { describe, it, expect, beforeEach } from "vitest";
import {
  getLastOpenedDocId,
  setLastOpenedDocId,
  clearLastOpenedDocId,
} from "../lib/last-opened";

describe("last-opened doc id slot", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("returns null when nothing is stored", () => {
    expect(getLastOpenedDocId()).toBeNull();
  });

  it("round-trips a written value", () => {
    setLastOpenedDocId("abc123");
    expect(getLastOpenedDocId()).toBe("abc123");
  });

  it("clears the slot", () => {
    setLastOpenedDocId("abc123");
    clearLastOpenedDocId();
    expect(getLastOpenedDocId()).toBeNull();
  });

  it("treats setLastOpenedDocId(null) as a clear", () => {
    setLastOpenedDocId("abc123");
    setLastOpenedDocId(null);
    expect(getLastOpenedDocId()).toBeNull();
  });

  it("treats empty-string slot as null (so getMostRecent fallback fires)", () => {
    window.localStorage.setItem("vcad:last-opened-doc-id", "");
    expect(getLastOpenedDocId()).toBeNull();
  });

  it("does not throw if localStorage is unavailable", () => {
    // Replace localStorage with one that throws on every method.
    const original = window.localStorage;
    const throwing = new Proxy(
      {},
      {
        get() {
          throw new Error("storage disabled");
        },
      },
    ) as unknown as Storage;
    Object.defineProperty(window, "localStorage", {
      value: throwing,
      configurable: true,
    });
    try {
      expect(() => getLastOpenedDocId()).not.toThrow();
      expect(() => setLastOpenedDocId("x")).not.toThrow();
      expect(() => clearLastOpenedDocId()).not.toThrow();
      expect(getLastOpenedDocId()).toBeNull();
    } finally {
      Object.defineProperty(window, "localStorage", {
        value: original,
        configurable: true,
      });
    }
  });
});
