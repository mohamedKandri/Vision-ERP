import { describe, expect, it } from "vitest";
import type { Order } from "./order";
import { buildReceipt, receiptText, row, wrap } from "./receipt";

const settings = {
  shop_name: "Burger Lab Casa", shop_address: "12 Bd Anfa\nCasablanca", shop_phone: "0522 00 00 00",
  shop_ice: "001234567000089", shop_if: "1234567", shop_rc: "", shop_patente: "", shop_logo: "",
  receipt_header: "", receipt_footer: "Merci et à bientôt !", receipt_show_qr: true,
};

const order: Order = {
  id: 7, number: 42, split_index: 0, parent_id: null, type: "dine_in", table_id: 3, table_name: "T3",
  customer_name: "", customer_phone: "", customer_address: "", delivery_fee_cents: 0, note: "",
  status: "sent", payment_status: "paid", held: false, hold_label: "",
  discount_kind: "percent", discount_value: 1000, discount_reason: "habitué",
  subtotal_cents: 9500, discount_cents: 950, base_cents: 7773, tax_cents: 777, total_cents: 8550, paid_cents: 8550, due_cents: 0,
  vat: [{ tax_id: 1, rate_bp: 1000, base: 7773, tax: 777, total: 8550 }],
  register: "main", created_by_name: "Salma", created_at: "2026-11-13T12:30:00.000Z", paid_at: null, refunded_at: null, voided_at: null,
  lines: [
    { id: 1, product_id: 1, variant_id: 2, config: { product_id: 1, variant_id: 2, modifiers: [] }, names: { fr: "Classic Burger", ar: "برغر كلاسيك", en: "Classic Burger" },
      details: { fr: ["Double", "+ Fromage ×2"], ar: ["مزدوج", "+ جبن ×2"], en: ["Double", "+ Cheese ×2"] }, qty: 1, unit_cents: 7000, happy_hour: false,
      tax_id: 1, tax_rate_bp: 1000, price_include: true, discount_kind: null, discount_value: null, discount_reason: null,
      gross_cents: 7000, discount_cents: 700, base_cents: 5727, tax_cents: 573, total_cents: 6300, note: "bien cuit", station_id: 1,
      kitchen_status: "sent", voided: false, void_reason: null },
    { id: 2, product_id: 5, variant_id: null, config: { product_id: 5, variant_id: null, modifiers: [] }, names: { fr: "Tiramisu" },
      details: { fr: [], ar: [], en: [] }, qty: 1, unit_cents: 2500, happy_hour: false, tax_id: 1, tax_rate_bp: 1000, price_include: true,
      discount_kind: null, discount_value: null, discount_reason: null, gross_cents: 2500, discount_cents: 250, base_cents: 2045, tax_cents: 205,
      total_cents: 2250, note: "", station_id: 4, kitchen_status: "sent", voided: false, void_reason: null },
    { id: 3, product_id: 6, variant_id: null, config: { product_id: 6, variant_id: null, modifiers: [] }, names: { fr: "Cookie" },
      details: { fr: [], ar: [], en: [] }, qty: 1, unit_cents: 1200, happy_hour: false, tax_id: 1, tax_rate_bp: 1000, price_include: true,
      discount_kind: null, discount_value: null, discount_reason: null, gross_cents: 0, discount_cents: 0, base_cents: 0, tax_cents: 0,
      total_cents: 0, note: "", station_id: 4, kitchen_status: "sent", voided: true, void_reason: "erreur" },
  ],
  payments: [{ id: 1, kind: "payment", method: "cash", amount_cents: 8550, tendered_cents: 10000, change_cents: 1450, at: "2026-11-13T12:31:00.000Z" }],
};

const fixedDate = () => "13/11/2026 13:30";

describe("wrap / row", () => {
  it("never exceeds the width", () => {
    for (const w of [32, 48]) {
      for (const l of wrap("Un très long nom de produit qui ne tient pas sur une seule ligne du ticket", w)) expect(l.length).toBeLessThanOrEqual(w);
      for (const l of row("2 x Menu Signature avec un nom vraiment très long", "1 234,00 DH", w)) expect(l.length).toBeLessThanOrEqual(w);
    }
  });
  it("aligns the amount to the right edge", () => {
    const [l] = row("TOTAL", "85,50 DH", 32);
    expect(l).toHaveLength(32);
    expect(l.endsWith("85,50 DH")).toBe(true);
    expect(l.startsWith("TOTAL")).toBe(true);
  });
  it("breaks words longer than the width", () => {
    expect(wrap("x".repeat(70), 32)).toEqual(["x".repeat(32), "x".repeat(32), "x".repeat(6)]);
  });
});

describe("buildReceipt", () => {
  const lines = buildReceipt(order, settings, { lang: "fr", width: 32, formatDate: fixedDate });
  const all = receiptText(lines, 32).join("\n").replace(/[  ]/g, " ");

  it("has shop info, legal ids, order number, table and cashier", () => {
    expect(all).toContain("Burger Lab Casa");
    expect(all).toContain("Casablanca");
    expect(all).toContain("ICE 001234567000089");
    expect(all).toContain("IF 1234567");
    expect(all).not.toContain("RC ");
    expect(all).toContain("Commande #42");
    expect(all).toContain("Sur place · Table T3");
    expect(all).toContain("Caissier: Salma");
    expect(all).toContain("13/11/2026 13:30");
  });
  it("lists items with details and notes, skips voided lines", () => {
    expect(all).toMatch(/1 x Classic Burger\s+70,00 DH/);
    expect(all).toContain("+ Fromage ×2");
    expect(all).toContain("Note: bien cuit");
    expect(all).not.toContain("Cookie");
  });
  it("shows subtotal, order discount, total, VAT, payment and change", () => {
    expect(all).toMatch(/Sous-total\s+95,00 DH/);
    expect(all).toMatch(/Remise 10 %\s+-9,50 DH/);
    expect(all).toMatch(/TOTAL TTC\s+85,50 DH/);
    expect(all).toMatch(/TVA 10 % HT 77,73 DH\s+7,77 DH/);
    expect(all).toMatch(/Espèces \(Reçu\)\s+100,00 DH/);
    expect(all).toMatch(/Rendu\s+14,50 DH/);
    expect(all).toContain("Merci et à bientôt !");
    expect(lines.some((l) => l.kind === "qr")).toBe(true);
  });
  it("every text line fits 58 mm and 80 mm paper", () => {
    for (const w of [32, 48] as const)
      for (const l of receiptText(buildReceipt(order, settings, { lang: "fr", width: w, formatDate: fixedDate }), w)) expect(l.length).toBeLessThanOrEqual(w);
  });
  it("marks reprints, refunds and balance due", () => {
    const re = receiptText(buildReceipt({ ...order, payment_status: "refunded" }, settings, { lang: "fr", width: 32, reprint: true, formatDate: fixedDate }), 32).join("\n");
    expect(re).toContain("*** DUPLICATA ***");
    expect(re).toContain("*** REMBOURSÉ ***");
    const partial = receiptText(
      buildReceipt({ ...order, payments: [], paid_cents: 0, due_cents: 8550, payment_status: "unpaid" }, settings, { lang: "fr", width: 32, formatDate: fixedDate }),
      32,
    ).join("\n");
    expect(partial).toMatch(/Reste à payer\s+85,50 DH/);
  });
  it("uses the receipt language for names, details and labels", () => {
    const ar = receiptText(buildReceipt(order, settings, { lang: "ar", width: 48, formatDate: fixedDate }), 48).join("\n");
    expect(ar).toContain("برغر كلاسيك");
    expect(ar).toContain("+ جبن ×2");
    expect(ar).toContain("المجموع");
    const en = receiptText(buildReceipt(order, settings, { lang: "en", width: 48, formatDate: fixedDate }), 48).join("\n");
    expect(en).toContain("Dine-in · Table T3");
    expect(en).toMatch(/Change\s+14.50 DH/);
  });
  it("delivery shows customer block and fee; comps show as free", () => {
    const d: Order = {
      ...order, type: "delivery", table_name: null, customer_name: "Sara", customer_phone: "0600000000", customer_address: "5 rue des Lilas, Maarif",
      delivery_fee_cents: 1000, subtotal_cents: 10500, discount_kind: null, discount_value: null, total_cents: 10500,
      lines: [{ ...order.lines[0], discount_kind: "comp", discount_reason: "anniversaire" }, order.lines[1]],
    };
    const txt = receiptText(buildReceipt(d, settings, { lang: "fr", width: 32, formatDate: fixedDate }), 32).join("\n");
    expect(txt).toContain("Livraison");
    expect(txt).toContain("0600000000");
    expect(txt).toMatch(/Frais de livraison\s+10,00 DH/);
    expect(txt).toMatch(/Offert \(anniversaire\)\s+-70,00 DH/);
  });
});
