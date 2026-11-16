import { describe, expect, it } from "vitest";
import { lineValue, packName, poTotal, remaining, unitCostOfPack } from "./purchasing";

describe("purchasing", () => {
  it("values partial packs like the Host", () => {
    expect(lineValue(2_500_000, 5_000_000, 45_000)).toBe(22_500);
    expect(lineValue(1000, 3000, 100)).toBe(33);
    expect(lineValue(2000, 3000, 100)).toBe(67);
    expect(lineValue(5, 0, 100)).toBe(0);
  });
  it("computes cost per kg / piece from a pack", () => {
    expect(unitCostOfPack("g", 5_000_000, 45_000)).toBe(9000); // 450 for 5 kg = 90/kg
    expect(unitCostOfPack("piece", 24_000, 6000)).toBe(250); // 60 for 24 = 2.50
    expect(unitCostOfPack("ml", 5_000_000, 20_000)).toBe(4000);
  });
  it("totals, remaining and pack names", () => {
    expect(poTotal([{ qty_packs: 4, pack_cost_cents: 45_000 }, { qty_packs: 1, pack_cost_cents: 999 }])).toBe(180_999);
    expect(remaining(20_000, 12_000)).toBe(8000);
    expect(remaining(20_000, 25_000)).toBe(0);
    expect(packName("Sachet 5 kg", 5_000_000, "g", "fr")).toBe("Sachet 5 kg");
    expect(packName(" ", 5_000_000, "g", "en")).toBe("5 kg");
  });
});
