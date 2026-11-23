import { describe, expect, it } from "vitest";
import { formatPhone, normalizePhone, pointsValue, stampProgress } from "./loyalty";

describe("loyalty helpers (same rules as the Host)", () => {
  it("normalises phone numbers", () => {
    expect(normalizePhone("06 12 34 56 78")).toBe("0612345678");
    expect(normalizePhone("+212 6-12-34-56-78")).toBe("0612345678");
    expect(normalizePhone("00212612345678")).toBe("0612345678");
    expect(normalizePhone("212612345678")).toBe("0612345678");
    expect(normalizePhone("+33 6 12 34 56 78")).toBe("33612345678");
    expect(normalizePhone("0522.11.22.33")).toBe("0522112233");
    expect(normalizePhone("12345")).toBeNull();
    expect(normalizePhone("abc")).toBeNull();
    expect(normalizePhone("1234567890123456789")).toBeNull();
  });
  it("formats and counts", () => {
    expect(formatPhone("0612345678")).toBe("06 12 34 56 78");
    expect(formatPhone("33612345678")).toBe("33612345678");
    expect(stampProgress(13, 10)).toEqual({ rewards: 1, current: 3 });
    expect(stampProgress(9, 10)).toEqual({ rewards: 0, current: 9 });
    expect(stampProgress(-2, 10)).toEqual({ rewards: 0, current: 0 });
    expect(pointsValue(68, 10)).toBe(680);
  });
});
