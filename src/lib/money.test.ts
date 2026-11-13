import { describe, expect, it } from "vitest";
import {
  allocate,
  changeDue,
  computeLine,
  computeOrder,
  discountAmount,
  divRound,
  formatMoney,
  formatRate,
  parseMoney,
  percentOf,
  splitIncluded,
  type Tax,
} from "./money";

const TTC10: Tax = { id: 1, rate_bp: 1000, price_include: true };
const TTC20: Tax = { id: 2, rate_bp: 2000, price_include: true };
const HT10: Tax = { id: 3, rate_bp: 1000, price_include: false };
const ZERO: Tax = { id: 4, rate_bp: 0, price_include: true };

describe("divRound (half away from zero)", () => {
  it.each([
    [5, 2, 3],
    [-5, 2, -3],
    [5, -2, -3],
    [4, 3, 1],
    [5, 3, 2],
    [0, 7, 0],
    [7, 7, 1],
    [1, 3, 0],
    [3, 2, 2],
    [-3, 2, -2],
  ])("%i / %i = %i", (n, d, q) => expect(divRound(n, d)).toBe(q));
  it("rejects zero and non-integers", () => {
    expect(() => divRound(1, 0)).toThrow();
    expect(() => divRound(1.5, 2)).toThrow();
  });
});

describe("VAT split", () => {
  it("45.00 TTC at 10% = 40.91 HT + 4.09 VAT", () => {
    expect(splitIncluded(4500, 1000)).toEqual({ base: 4091, tax: 409 });
  });
  it("12.00 TTC at 20% = 10.00 + 2.00", () => {
    expect(splitIncluded(1200, 2000)).toEqual({ base: 1000, tax: 200 });
  });
  it("0% VAT keeps everything as base", () => {
    expect(splitIncluded(999, 0)).toEqual({ base: 999, tax: 0 });
  });
  it("1 centime at 10% rounds base up to 1, tax 0", () => {
    expect(splitIncluded(1, 1000)).toEqual({ base: 1, tax: 0 });
  });
  it("percentOf rounds half up", () => {
    expect(percentOf(5, 1000)).toBe(1); // 0.5 → 1
    expect(percentOf(4, 1000)).toBe(0); // 0.4 → 0
    expect(percentOf(4091, 1000)).toBe(409); // 409.1
    expect(percentOf(-5, 1000)).toBe(-1);
  });
});

describe("computeLine", () => {
  it("TTC product: total is the shelf price", () => {
    expect(computeLine({ unit_cents: 4500, qty: 2, tax: TTC10 })).toEqual({
      gross: 9000, discount: 0, base: 8182, tax: 818, total: 9000,
    });
  });
  it("HT product: VAT is added on top", () => {
    expect(computeLine({ unit_cents: 4091, qty: 1, tax: HT10 })).toEqual({
      gross: 4091, discount: 0, base: 4091, tax: 409, total: 4500,
    });
  });
  it("percent discount on a line", () => {
    const l = computeLine({ unit_cents: 4500, qty: 1, tax: TTC10, discount: { kind: "percent", bp: 1000 } });
    expect(l.discount).toBe(450);
    expect(l.total).toBe(4050);
    expect(l.base + l.tax).toBe(4050);
  });
  it("fixed discount larger than the line is capped (free item)", () => {
    const l = computeLine({ unit_cents: 1500, qty: 1, tax: TTC10, discount: { kind: "fixed", cents: 99999 } });
    expect(l).toEqual({ gross: 1500, discount: 1500, base: 0, tax: 0, total: 0 });
  });
  it("100% discount = comp", () => {
    expect(computeLine({ unit_cents: 4500, qty: 3, tax: TTC10, discount: { kind: "percent", bp: 10000 } }).total).toBe(0);
  });
  it("rejects fractional cents, negative qty, bad discounts", () => {
    expect(() => computeLine({ unit_cents: 10.5, qty: 1, tax: TTC10 })).toThrow();
    expect(() => computeLine({ unit_cents: 100, qty: -1, tax: TTC10 })).toThrow();
    expect(() => discountAmount(100, { kind: "percent", bp: 10001 })).toThrow();
    expect(() => discountAmount(100, { kind: "fixed", cents: -1 })).toThrow();
  });
  it("qty 0 is allowed and yields zero", () => {
    expect(computeLine({ unit_cents: 4500, qty: 0, tax: TTC10 }).total).toBe(0);
  });
});

describe("allocate", () => {
  it("sums exactly and favours largest remainders", () => {
    expect(allocate(100, [1, 1, 1])).toEqual([34, 33, 33]);
    expect(allocate(600, [4500, 1500])).toEqual([450, 150]);
    expect(allocate(1, [1, 1])).toEqual([1, 0]);
  });
  it("handles negatives, zero weights and empty lists", () => {
    expect(allocate(-100, [1, 1, 1])).toEqual([-34, -33, -33]);
    expect(allocate(50, [0, 0])).toEqual([50, 0]);
    expect(allocate(50, [])).toEqual([]);
  });
  it("random sums always match", () => {
    for (let i = 0; i < 500; i++) {
      const w = Array.from({ length: 1 + (i % 7) }, (_, k) => ((i * 31 + k * 17) % 9000) + 1);
      const amount = (i * 97) % 10000;
      expect(allocate(amount, w).reduce((a, b) => a + b, 0)).toBe(amount);
    }
  });
});

describe("computeOrder", () => {
  it("VAT breakdown per rate", () => {
    const o = computeOrder([
      { unit_cents: 4500, qty: 1, tax: TTC10 },
      { unit_cents: 1200, qty: 1, tax: TTC20 },
    ]);
    expect(o.total).toBe(5700);
    expect(o.vat).toEqual([
      { tax_id: 1, rate_bp: 1000, base: 4091, tax: 409, total: 4500 },
      { tax_id: 2, rate_bp: 2000, base: 1000, tax: 200, total: 1200 },
    ]);
    expect(o.base + o.tax).toBe(o.total);
  });

  it("per-line vs global rounding can differ by a centime", () => {
    const lines = [1, 2, 3].map(() => ({ unit_cents: 105, qty: 1, tax: TTC10 }));
    const perLine = computeOrder(lines, { rounding: "per_line" });
    const global = computeOrder(lines, { rounding: "global" });
    expect(perLine.tax).toBe(30); // 3 × 10
    expect(global.tax).toBe(29); // 315 → 286 + 29
    expect(perLine.total).toBe(315);
    expect(global.total).toBe(315); // TTC prices: customer pays the same either way
  });

  it("order discount is spread over lines and VAT recomputed", () => {
    const o = computeOrder(
      [
        { unit_cents: 4500, qty: 1, tax: TTC10 },
        { unit_cents: 1500, qty: 1, tax: TTC20 },
      ],
      { orderDiscount: { kind: "percent", bp: 1000 } },
    );
    expect(o.subtotal).toBe(6000);
    expect(o.order_discount).toBe(600);
    expect(o.total).toBe(5400);
    expect(o.lines.map((l) => l.total)).toEqual([4050, 1350]);
    expect(o.vat[0]).toEqual({ tax_id: 1, rate_bp: 1000, base: 3682, tax: 368, total: 4050 });
    expect(o.vat[1]).toEqual({ tax_id: 2, rate_bp: 2000, base: 1125, tax: 225, total: 1350 });
  });

  it("fixed order discount bigger than the order makes it free, never negative", () => {
    const o = computeOrder([{ unit_cents: 4500, qty: 1, tax: TTC10 }], { orderDiscount: { kind: "fixed", cents: 10000 } });
    expect(o.total).toBe(0);
    expect(o.tax).toBe(0);
  });

  it("mixed HT and TTC products", () => {
    const o = computeOrder([
      { unit_cents: 4500, qty: 1, tax: TTC10 },
      { unit_cents: 1000, qty: 2, tax: HT10 },
    ]);
    expect(o.total).toBe(4500 + 2200);
    expect(o.vat).toHaveLength(2);
    expect(o.base + o.tax).toBe(o.total);
  });

  it("delivery fee is taxed and added", () => {
    const o = computeOrder([{ unit_cents: 4500, qty: 1, tax: TTC10 }], { extraFee: { cents: 1000, tax: TTC20 } });
    expect(o.total).toBe(5500);
    expect(o.vat.find((b) => b.tax_id === 2)).toEqual({ tax_id: 2, rate_bp: 2000, base: 833, tax: 167, total: 1000 });
  });

  it("empty order is zero", () => {
    const o = computeOrder([]);
    expect(o).toMatchObject({ subtotal: 0, total: 0, base: 0, tax: 0, vat: [] });
  });

  it("invariants hold on many random orders", () => {
    const taxes = [TTC10, TTC20, HT10, ZERO];
    for (let i = 0; i < 400; i++) {
      const lines = Array.from({ length: 1 + (i % 6) }, (_, k) => ({
        unit_cents: ((i * 131 + k * 71) % 9000) + 50,
        qty: 1 + ((i + k) % 4),
        tax: taxes[(i + k) % taxes.length],
        discount: k % 3 === 0 ? ({ kind: "percent", bp: (i * 13) % 3000 } as const) : null,
      }));
      for (const rounding of ["per_line", "global"] as const) {
        const o = computeOrder(lines, { rounding, orderDiscount: i % 2 ? { kind: "percent", bp: 500 } : null });
        expect(o.base + o.tax).toBe(o.total);
        expect(o.total).toBeGreaterThanOrEqual(0);
        expect(o.vat.reduce((a, b) => a + b.total, 0)).toBe(o.total);
        if (rounding === "per_line") expect(o.lines.reduce((a, l) => a + l.total, 0)).toBe(o.total);
      }
    }
  });
});

describe("cash", () => {
  it("change due", () => {
    expect(changeDue(5700, 10000)).toBe(4300);
    expect(changeDue(5700, 5700)).toBe(0);
    expect(() => changeDue(5700, 5000)).toThrow("insufficient");
  });
});

describe("parseMoney", () => {
  it.each([
    ["45", 4500],
    ["45,5", 4550],
    ["45.50", 4550],
    ["0,05", 5],
    ["1 200,00", 120000],
    ["12 DH", 1200],
    ["-3,20", -320],
  ])("%s → %i", (s, c) => expect(parseMoney(s)).toBe(c));
  it.each(["", "abc", "4,555", "4,5,5", "--1"])("rejects %s", (s) => expect(parseMoney(s)).toBeNull());
});

describe("format", () => {
  it("money in each language with Latin digits", () => {
    expect(formatMoney(4500, "en")).toBe("45.00 DH");
    expect(formatMoney(4500, "fr")).toMatch(/^45,00 DH$/);
    expect(formatMoney(4500, "ar")).toMatch(/45.00 د\.م\.$/);
    expect(formatMoney(123456, "fr").replace(/\s/g, " ")).toMatch(/^1.234,56 DH$/);
  });
  it("rates", () => {
    expect(formatRate(1000)).toBe("10 %");
    expect(formatRate(550)).toBe("5.5 %");
  });
});
