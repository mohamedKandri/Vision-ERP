// Shared pricing test vectors. The same JSON file is checked by the Rust pricing tests,
// proving the Host (Rust) and the tills (TypeScript) compute identical totals.
// Regenerate after an intentional pricing change:  UPDATE_VECTORS=1 pnpm test
import { readFileSync, writeFileSync, existsSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { computeOrder, type Discount, type LineInput, type Tax, type TaxRounding } from "./money";

const FILE = new URL("../../test-vectors/pricing.json", import.meta.url);

interface Case {
  name: string;
  lines: LineInput[];
  opts: { rounding: TaxRounding; orderDiscount: Discount; extraFee?: { cents: number; tax: Tax } };
  expected?: ReturnType<typeof computeOrder>;
}

const T: Tax[] = [
  { id: 1, rate_bp: 1000, price_include: true },
  { id: 2, rate_bp: 2000, price_include: true },
  { id: 3, rate_bp: 1000, price_include: false },
  { id: 4, rate_bp: 0, price_include: true },
  { id: 5, rate_bp: 700, price_include: false },
  { id: 6, rate_bp: 550, price_include: true },
];

function cases(): Case[] {
  const out: Case[] = [
    { name: "single burger", lines: [{ unit_cents: 4500, qty: 1, tax: T[0] }], opts: { rounding: "per_line", orderDiscount: null } },
    { name: "rounding differs", lines: [1, 2, 3].map(() => ({ unit_cents: 105, qty: 1, tax: T[0] })), opts: { rounding: "global", orderDiscount: null } },
    { name: "comp line", lines: [{ unit_cents: 4500, qty: 2, tax: T[0], discount: { kind: "percent", bp: 10000 } }, { unit_cents: 1200, qty: 1, tax: T[1] }], opts: { rounding: "per_line", orderDiscount: null } },
    { name: "fixed order discount over total", lines: [{ unit_cents: 4500, qty: 1, tax: T[0] }], opts: { rounding: "per_line", orderDiscount: { kind: "fixed", cents: 99999 } } },
    { name: "delivery fee", lines: [{ unit_cents: 6000, qty: 1, tax: T[0] }], opts: { rounding: "per_line", orderDiscount: { kind: "percent", bp: 1000 }, extraFee: { cents: 1000, tax: T[1] } } },
    { name: "empty", lines: [], opts: { rounding: "per_line", orderDiscount: null } },
  ];
  // Deterministic pseudo-random cases.
  let seed = 42;
  const rnd = (n: number) => ((seed = (seed * 1103515245 + 12345) % 2147483648), seed % n);
  for (let i = 0; i < 300; i++) {
    const lines: LineInput[] = Array.from({ length: 1 + rnd(7) }, () => {
      const d = rnd(5);
      const discount: Discount =
        d === 0 ? { kind: "percent", bp: rnd(10001) } : d === 1 ? { kind: "fixed", cents: rnd(5000) } : null;
      return { unit_cents: rnd(12000), qty: 1 + rnd(5), tax: T[rnd(T.length)], discount };
    });
    const od = rnd(4);
    const orderDiscount: Discount = od === 0 ? { kind: "percent", bp: rnd(5001) } : od === 1 ? { kind: "fixed", cents: rnd(3000) } : null;
    const extraFee = rnd(3) === 0 ? { cents: rnd(3000), tax: T[rnd(T.length)] } : undefined;
    out.push({ name: `random ${i}`, lines, opts: { rounding: rnd(2) ? "global" : "per_line", orderDiscount, extraFee } });
  }
  return out.map((c) => ({ ...c, expected: computeOrder(c.lines, c.opts) }));
}

describe("shared pricing vectors", () => {
  it("TypeScript matches test-vectors/pricing.json", () => {
    const fresh = cases();
    if (process.env.UPDATE_VECTORS || !existsSync(FILE)) writeFileSync(FILE, JSON.stringify(fresh, null, 1));
    const stored: Case[] = JSON.parse(readFileSync(FILE, "utf8"));
    expect(stored.length).toBe(fresh.length);
    for (const c of stored) expect(computeOrder(c.lines, c.opts), c.name).toEqual(c.expected);
  });
});
