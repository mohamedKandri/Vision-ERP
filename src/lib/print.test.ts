import { describe, expect, it } from "vitest";
import { reportDoc, type CashReportLike } from "./print";
import { receiptText } from "./receipt";

const z: CashReportLike = {
  register: "main", z_number: 3, opened_by: "Salma", opened_at: "2026-11-13T08:00:00Z", closed_at: "2026-11-13T22:00:00Z",
  float_cents: 50000, cash_sales_cents: 9700, card_sales_cents: 5850, cash_refunds_cents: 0, card_refunds_cents: 0,
  cash_in_cents: 0, cash_out_cents: 2000, expected_cash_cents: 57700, counted_cash_cents: 57500, difference_cents: -200,
  orders: 2, average_ticket_cents: 7775, discounts_cents: 650, comps_cents: 0, voids: 1, voids_total_cents: 2500, refunds: 0,
  net_sales_cents: 15550, by_type: [{ type: "takeaway", count: 1, total_cents: 9700 }],
  vat: [{ tax_id: 1, rate_bp: 1000, base: 14136, tax: 1414 }],
  movements: [{ kind: "out", amount_cents: 2000, reason: "Achat pain", user: "Salma" }],
};

describe("reportDoc", () => {
  const txt = receiptText(reportDoc(z, "Burger Lab", "fr", () => "13/11"), 48).join("\n").replace(/[  ]/g, " ");
  it("prints Z number, cash block and difference", () => {
    expect(txt).toContain("Rapport Z n°3");
    expect(txt).toMatch(/Espèces attendues\s+577,00 DH/);
    expect(txt).toMatch(/Espèces comptées\s+575,00 DH/);
    expect(txt).toMatch(/Écart\s+-2,00 DH/);
  });
  it("prints sales, VAT and movements", () => {
    expect(txt).toMatch(/Ventes nettes\s+155,50 DH/);
    expect(txt).toMatch(/10 % HT 141,36 DH\s+14,14 DH/);
    expect(txt).toMatch(/- Achat pain \(Salma\)\s+20,00 DH/);
    expect(txt).toMatch(/À emporter \(1\)\s+97,00 DH/);
  });
  it("X report has no counted/difference lines and fits 58 mm", () => {
    const x = reportDoc({ ...z, z_number: null, closed_at: null, counted_cash_cents: null, difference_cents: null }, "Burger Lab", "fr", () => "13/11");
    const t = receiptText(x, 32);
    expect(t.join("\n")).toContain("Rapport X");
    expect(t.join("\n")).not.toContain("comptées");
    for (const l of t) expect(l.length).toBeLessThanOrEqual(32);
  });
});
