/**
 * Tests for the shared export-control geo-block module (shared/geo-block.ts).
 *
 * Lives here (rather than next to the module) because packages/mcp already
 * has vitest wired up and src/__tests__ is excluded from the tsc build, so
 * the cross-package relative import never reaches the compiler output.
 */

import { describe, expect, it } from "vitest";
import {
  BLOCKED_COUNTRIES,
  GEO_BLOCK_BODY,
  GEO_BLOCK_STATUS,
  geoBlockResponse,
  isGeoBlocked,
  isRequestGeoBlocked,
} from "../../../../shared/geo-block";

describe("isGeoBlocked", () => {
  it("blocks every listed country", () => {
    for (const cc of ["RU", "BY", "IR", "CU", "KP", "SY"]) {
      expect(isGeoBlocked(cc), cc).toBe(true);
    }
  });

  it("is case- and whitespace-insensitive on the country code", () => {
    expect(isGeoBlocked("ru")).toBe(true);
    expect(isGeoBlocked(" RU ")).toBe(true);
  });

  it("allows US and other unlisted countries", () => {
    expect(isGeoBlocked("US")).toBe(false);
    expect(isGeoBlocked("DE")).toBe(false);
    expect(isGeoBlocked("JP")).toBe(false);
  });

  it("fails open when the country header is missing", () => {
    expect(isGeoBlocked(null)).toBe(false);
    expect(isGeoBlocked(undefined)).toBe(false);
    expect(isGeoBlocked("")).toBe(false);
  });

  it("allows Ukraine generally (e.g. Kyiv)", () => {
    expect(isGeoBlocked("UA")).toBe(false);
    expect(isGeoBlocked("UA", "30")).toBe(false); // Kyiv city
    expect(isGeoBlocked("UA", null)).toBe(false); // region header missing
  });

  it("blocks the occupied Ukrainian regions", () => {
    expect(isGeoBlocked("UA", "43")).toBe(true); // Crimea
    expect(isGeoBlocked("UA", "40")).toBe(true); // Sevastopol
    expect(isGeoBlocked("UA", "14")).toBe(true); // Donetsk
    expect(isGeoBlocked("UA", "09")).toBe(true); // Luhansk
  });

  it("accepts the full ISO 3166-2 'UA-43' region form", () => {
    expect(isGeoBlocked("UA", "UA-43")).toBe(true);
    expect(isGeoBlocked("ua", "ua-40")).toBe(true);
  });

  it("does not apply UA region codes to other countries", () => {
    // "43" is only meaningful as a subdivision of UA.
    expect(isGeoBlocked("US", "43")).toBe(false);
  });
});

describe("isRequestGeoBlocked", () => {
  const req = (headers: Record<string, string>) =>
    new Request("https://vcad.io/", { headers });

  it("blocks based on the x-vercel-ip-country header", () => {
    expect(isRequestGeoBlocked(req({ "x-vercel-ip-country": "RU" }))).toBe(
      true,
    );
    expect(isRequestGeoBlocked(req({ "x-vercel-ip-country": "US" }))).toBe(
      false,
    );
  });

  it("combines country and region headers", () => {
    expect(
      isRequestGeoBlocked(
        req({
          "x-vercel-ip-country": "UA",
          "x-vercel-ip-country-region": "43",
        }),
      ),
    ).toBe(true);
    expect(
      isRequestGeoBlocked(
        req({
          "x-vercel-ip-country": "UA",
          "x-vercel-ip-country-region": "30",
        }),
      ),
    ).toBe(false);
  });

  it("fails open with no geo headers at all", () => {
    expect(isRequestGeoBlocked(req({}))).toBe(false);
  });

  it("honors request.geo when the platform provides it", () => {
    const r = req({}) as Request & { geo?: { country?: string } };
    r.geo = { country: "RU" };
    expect(isRequestGeoBlocked(r)).toBe(true);
  });
});

describe("geoBlockResponse", () => {
  it("returns HTTP 451 with the shared JSON body", async () => {
    const res = geoBlockResponse();
    expect(res.status).toBe(GEO_BLOCK_STATUS);
    expect(res.status).toBe(451);
    expect(res.headers.get("content-type")).toBe("application/json");
    const body = await res.text();
    expect(body).toBe(GEO_BLOCK_BODY);
    const parsed = JSON.parse(body) as { message: string };
    expect(parsed.message).toContain("export controls");
    expect(parsed.message).toContain("github.com/ecto/vcad");
  });
});

describe("block list", () => {
  it("matches the jurisdictions in the OFAC/BIS citations", () => {
    expect([...BLOCKED_COUNTRIES].sort()).toEqual([
      "BY",
      "CU",
      "IR",
      "KP",
      "RU",
      "SY",
    ]);
  });
});
