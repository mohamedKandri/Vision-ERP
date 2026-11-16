import { describe, expect, it } from "vitest";
import { foodCostBp, formatQty, marginBp, marginLevel, mcToCents, parseQty, priceHt, qtyInput, recipeCost, valueMc, type Unit } from "./stock";

describe("parseQty", () => {
  it("reads quantities in the base unit with up to 3 decimals", () => {
    expect(parseQty("150")).toBe(150_000);
    expect(parseQty("1,5")).toBe(1500);
    expect(parseQty(" 0.250 ")).toBe(250);
    expect(parseQty(".5")).toBe(500);
    expect(parseQty("1 200")).toBe(1_200_000);
    expect(parseQty("0")).toBe(0);
  });
  it("rejects junk, too many decimals and negatives unless allowed", () => {
    expect(parseQty("")).toBeNull();
    expect(parseQty("abc")).toBeNull();
    expect(parseQty("1.2345")).toBeNull();
    expect(parseQty("1,2,3")).toBeNull();
    expect(parseQty("-20")).toBeNull();
    expect(parseQty("-20", true)).toBe(-20_000);
  });
  it("round-trips through qtyInput", () => {
    for (const v of [0, 1, 150_000, 1500, 250, -20_000, 1_234_567]) expect(parseQty(qtyInput(v), true)).toBe(v);
    expect(qtyInput(1500)).toBe("1.5");
    expect(qtyInput(150_000)).toBe("150");
  });
});

describe("formatQty", () => {
  it("switches to kg / L above 1000 and keeps pieces", () => {
    expect(formatQty(150_000, "g", "en")).toBe("150 g");
    expect(formatQty(2_500_000, "g", "en")).toBe("2.5 kg");
    expect(formatQty(1_000_000, "ml", "en")).toBe("1 L");
    expect(formatQty(12_000, "piece", "en")).toBe("12 pc");
    expect(formatQty(-500, "piece", "fr")).toMatch(/^-0,5 pce$/);
    expect(formatQty(12_000, "piece", "ar")).toBe("12 قطعة");
  });
});

describe("cost", () => {
  it("values quantities like the Host", () => {
    expect(mcToCents(valueMc("g", 9000, 150_000))).toBe(1350); // 150 g at 90/kg
    expect(mcToCents(valueMc("piece", 250, 1000))).toBe(250);
    expect(mcToCents(valueMc("piece", 121, 500))).toBe(61);
    expect(mcToCents(valueMc("ml", 4000, 20_000))).toBe(80);
    expect(mcToCents(valueMc("g", 9000, -150_000))).toBe(-1350);
  });
  it("costs the demo Classic Burger (Simple) at 18.71, like the Rust test", () => {
    const ing = new Map<number, { unit: Unit; cost_cents: number }>([
      [1, { unit: "piece", cost_cents: 250 }], // bun
      [2, { unit: "g", cost_cents: 9000 }], // beef
      [3, { unit: "g", cost_cents: 800 }], // onion
      [4, { unit: "g", cost_cents: 900 }], // tomato
      [5, { unit: "g", cost_cents: 1500 }], // lettuce
      [6, { unit: "g", cost_cents: 3000 }], // pickles
      [7, { unit: "ml", cost_cents: 4000 }], // sauce
      [8, { unit: "piece", cost_cents: 80 }], // box
    ]);
    const lines = [[1, 1000], [2, 150_000], [3, 20_000], [4, 30_000], [5, 15_000], [6, 15_000], [7, 20_000], [8, 1000]].map(([i, q]) => ({ ingredient_id: i, qty_milli: q }));
    expect(recipeCost(lines, ing)).toBe(1871);
    // A removal lowers the cost; an unknown ingredient counts 0.
    expect(recipeCost([...lines, { ingredient_id: 3, qty_milli: -20_000 }, { ingredient_id: 99, qty_milli: 5 }], ing)).toBe(1855);
  });
  it("computes margin and food cost on the price excluding VAT", () => {
    expect(priceHt(4500, 1000, true)).toBe(4091);
    expect(priceHt(4500, 1000, false)).toBe(4500);
    expect(marginBp(4091, 1600)).toBe(6089);
    expect(foodCostBp(4091, 1600)).toBe(3911);
    expect(marginBp(0, 100)).toBe(0);
    expect(marginBp(1000, 1500)).toBe(-5000);
    expect(marginLevel(7000)).toBe("good");
    expect(marginLevel(5500)).toBe("watch");
    expect(marginLevel(-100)).toBe("low");
  });
});
