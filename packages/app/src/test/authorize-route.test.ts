import { describe, it, expect } from "vitest";
import { parseAuthorizeRoute } from "../lib/authorize-route";

const ID = "6f9619ff-8b86-d011-b42d-00c04fc964ff";

describe("parseAuthorizeRoute", () => {
  it("extracts the authorization id from /authorize/<uuid>", () => {
    expect(parseAuthorizeRoute(`/authorize/${ID}`)).toBe(ID);
  });

  it("tolerates a trailing slash", () => {
    expect(parseAuthorizeRoute(`/authorize/${ID}/`)).toBe(ID);
  });

  it("accepts uppercase hex (uuids are case-insensitive)", () => {
    expect(parseAuthorizeRoute(`/authorize/${ID.toUpperCase()}`)).toBe(
      ID.toUpperCase(),
    );
  });

  it("rejects non-uuid ids so they fall through to the editor", () => {
    expect(parseAuthorizeRoute("/authorize/not-a-uuid")).toBeNull();
    expect(parseAuthorizeRoute("/authorize/")).toBeNull();
    expect(parseAuthorizeRoute("/authorize")).toBeNull();
    // sql-ish / traversal-ish junk never reaches the page
    expect(parseAuthorizeRoute(`/authorize/${ID}%20or%201=1`)).toBeNull();
    expect(parseAuthorizeRoute(`/authorize/../${ID}`)).toBeNull();
  });

  it("ignores unrelated routes", () => {
    expect(parseAuthorizeRoute("/")).toBeNull();
    expect(parseAuthorizeRoute("/cli-auth")).toBeNull();
    expect(parseAuthorizeRoute(`/view/${ID}`)).toBeNull();
    expect(parseAuthorizeRoute(`/authorized/${ID}`)).toBeNull();
    expect(parseAuthorizeRoute(`/x/authorize/${ID}`)).toBeNull();
  });
});
