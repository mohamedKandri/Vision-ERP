import { describe, expect, it } from "vitest";
import { buildPdf } from "./pdf";
import { isolateNumber } from "./reportPdf";
import {
  addDays, cellText, csvEscape, csvValue, dayKey, dayRange, exportName, parseDay, presetDays, reportTables, toBase64, toCsv,
  type Report, type Table,
} from "./report";

describe("date ranges", () => {
  it("turns local days into UTC instants", () => {
    const d = new Date(2026, 10, 17, 15, 30);
    const r = dayRange(d, d);
    const from = new Date(r.from);
    const to = new Date(r.to);
    expect([from.getHours(), from.getMinutes(), from.getDate()]).toEqual([0, 0, 17]);
    expect([to.getHours(), to.getDate()]).toEqual([0, 18]);
    expect(r.tz_offset_min).toBe(-from.getTimezoneOffset());
    expect(r.from.endsWith("Z")).toBe(true);
  });
  it("presets", () => {
    const now = new Date(2026, 10, 18, 10); // Wednesday 18 Nov 2026
    const k = (p: Parameters<typeof presetDays>[0]) => presetDays(p, now).map(dayKey);
    expect(k("today")).toEqual(["2026-11-18", "2026-11-18"]);
    expect(k("yesterday")).toEqual(["2026-11-17", "2026-11-17"]);
    expect(k("week")).toEqual(["2026-11-16", "2026-11-18"]); // from Monday
    expect(k("last7")).toEqual(["2026-11-12", "2026-11-18"]);
    expect(k("month")).toEqual(["2026-11-01", "2026-11-18"]);
    expect(k("last_month")).toEqual(["2026-10-01", "2026-10-31"]);
    expect(presetDays("week", new Date(2026, 10, 22)).map(dayKey)).toEqual(["2026-11-16", "2026-11-22"]); // Sunday
    expect(presetDays("last_month", new Date(2026, 0, 5)).map(dayKey)).toEqual(["2025-12-01", "2025-12-31"]);
  });
  it("parses days and builds file names", () => {
    expect(dayKey(parseDay("2026-02-28")!)).toBe("2026-02-28");
    expect(parseDay("2026-02-30")).toBeNull();
    expect(parseDay("17/11/2026")).toBeNull();
    expect(dayKey(addDays(new Date(2026, 11, 31), 1))).toBe("2027-01-01");
    const d = new Date(2026, 10, 17);
    expect(exportName("rapport", dayRange(d, d), "pdf")).toBe("rapport_2026-11-17.pdf");
    expect(exportName("rapport", dayRange(new Date(2026, 10, 1), d), "csv")).toBe("rapport_2026-11-01_2026-11-17.csv");
  });
});

describe("CSV", () => {
  it("formats numbers for the language's Excel", () => {
    expect(csvValue({ cents: 123456 }, "fr")).toBe("1234,56");
    expect(csvValue({ cents: -500 }, "en")).toBe("-5.00");
    expect(csvValue({ bp: 4573 }, "fr")).toBe("45,73");
    expect(csvValue({ bp: null }, "fr")).toBe("");
    expect(csvValue({ milli: 2_500_000, unit: "g" }, "fr")).toBe("2,500"); // kg
    expect(csvValue({ milli: 3000, unit: "piece" }, "en")).toBe("3.000");
    expect(csvValue({ int: 12 }, "ar")).toBe("12");
    expect(csvValue({ cents: -0 }, "en")).toBe("0.00");
  });
  it("escapes separators and quotes, and blocks formulas", () => {
    expect(csvEscape("a;b", ";")).toBe('"a;b"');
    expect(csvEscape('dit "ok"', ";")).toBe('"dit ""ok"""');
    expect(csvEscape("a,b", ";")).toBe("a,b");
    expect(csvValue({ text: "=HYPERLINK(1)" }, "fr")).toBe("'=HYPERLINK(1)");
    expect(csvValue({ text: "-10 % fidèle" }, "fr")).toBe("'-10 % fidèle");
    expect(csvValue({ text: "Pain burger" }, "fr")).toBe("Pain burger");
  });
  it("stacks tables in one file with a BOM", () => {
    const tb: Table = { id: "x", title: "Par produit", columns: [{ label: "Produit" }, { label: "Total", num: true }],
      rows: [[{ text: "Tiramisu" }, { cents: 2500 }], [{ text: "برغر" }, { cents: 100 }]], total: [{ text: "Total" }, { cents: 2600 }] };
    const csv = toCsv(["Test", "Du 17/11/2026"], [tb], "fr");
    expect(csv.startsWith("﻿Test\r\nDu 17/11/2026\r\n\r\nPar produit\r\nProduit;Total\r\nTiramisu;25,00\r\n")).toBe(true);
    expect(csv).toContain("برغر;1,00\r\nTotal;26,00\r\n");
  });
});

describe("cell text", () => {
  it("percent in the language's style, no negative zero", () => {
    expect(cellText({ bp: 7934 }, "fr")).toBe("79,3 %");
    expect(cellText({ bp: 7934 }, "en")).toBe("79.3 %");
    expect(cellText({ bp: 1000 }, "ar")).toBe("10 %");
    expect(cellText({ bp: null }, "fr")).toBe("—");
    expect(cellText({ cents: -0 }, "fr")).toBe(cellText({ cents: 0 }, "fr"));
    expect(cellText({ cents: -0 }, "fr").startsWith("-")).toBe(false);
  });
});

describe("report tables", () => {
  const empty: Report = {
    range: { from: "2026-11-16T23:00:00.000Z", to: "2026-11-17T23:00:00.000Z", tz_offset_min: 60 },
    totals: { orders: 3, refunds: 1, gross_cents: 12600, discount_cents: 500, base_cents: 6455, tax_cents: 645, net_cents: 7100, sales_cents: 12100, refunded_cents: 5000, delivery_fees_cents: 1000, avg_ticket_cents: 4033 },
    by_hour: Array.from({ length: 24 }, (_, hour) => ({ hour, orders: hour === 12 ? 3 : 0, total_cents: hour === 12 ? 7100 : 0 })),
    by_day: [{ day: "2026-11-17", orders: 3, total_cents: 7100 }],
    by_product: [{ product_id: 1, names: { fr: "Tiramisu" }, category_id: 2, kind: "simple", qty: 1, total_cents: 2500, base_cents: 2273, discount_cents: 0, cost_cents: 600, margin_cents: 1673, food_cost_bp: 2640 }],
    by_category: [], by_type: [{ type: "delivery", orders: 1, total_cents: 2200, avg_ticket_cents: 2200 }], by_staff: [],
    by_payment: [{ method: "cash", count: 2, paid_cents: 9900, refunded_cents: 5000, net_cents: 4900 }],
    vat: [], food_cost: { revenue_ht_cents: 2273, cost_cents: 600, margin_cents: 1673, food_cost_bp: 2640 },
    discounts: { count: 0, total_cents: 0, comp_cents: 0, items: [] },
    voids: { orders_cents: 1200, lines_cents: 0, refunds_cents: 5000,
      orders: [{ number: 5, split_index: 0, at: "2026-11-17T12:00:00.000Z", amount_cents: 1200, reason: "erreur", by: "Karim", created_by: "Salma" }],
      lines: [], refunds: [{ number: 3, split_index: 1, at: "2026-11-17T10:00:00.000Z", amount_cents: 5000, reason: "plainte", by: "Karim" }] },
    stock: { value_cents: 100000, ingredients: 49, moves: { sale: -600, return: 0, waste: -300, purchase: 5000 } },
    waste: { total_cents: 0, by_ingredient: [], items: [] },
  };
  const t = (k: string, v?: Record<string, string | number>) => (v ? `${k}(${Object.values(v).join(",")})` : k);
  const tables = reportTables(empty, t, "fr", (iso) => iso.slice(11, 16));
  const get = (id: string) => tables.find((x) => x.id === id)!;
  it("one table per section, totals that match the Host", () => {
    expect(tables.map((x) => x.id)).toEqual(["summary", "by_hour", "by_product", "by_category", "by_type", "by_payment", "by_staff", "vat", "discounts", "voids", "food_cost", "stock", "waste", "waste_items"]);
    expect(get("by_hour").rows).toEqual([[{ text: "12:00" }, { int: 3 }, { cents: 7100 }]]); // empty hours left out
    expect(get("by_product").total).toContainEqual({ bp: 2640 });
    expect(get("by_type").rows[0][0]).toEqual({ text: "order_type_delivery" });
    expect(get("by_payment").rows[0]).toEqual([{ text: "pos_cash" }, { int: 2 }, { cents: 9900 }, { cents: -5000 }, { cents: 4900 }]);
    expect(get("stock").rows[2][1]).toEqual({ cents: -600 });
  });
  it("voids and refunds in time order, split checks numbered", () => {
    const v = get("voids");
    expect(v.rows.map((r) => r[1])).toEqual([{ text: "#3-1" }, { text: "#5" }]);
    expect(v.total![5]).toEqual({ cents: 6200 });
  });
});

describe("PDF", () => {
  it("writes a valid file structure with one image per page", () => {
    const jpeg = new Uint8Array([0xff, 0xd8, 1, 2, 3, 0xff, 0xd9]);
    const pdf = buildPdf([{ jpeg, width: 10, height: 20 }, { jpeg, width: 10, height: 20 }]);
    const s = new TextDecoder("latin1").decode(pdf);
    expect(s.startsWith("%PDF-1.4\n%")).toBe(true);
    expect(s.trimEnd().endsWith("%%EOF")).toBe(true);
    expect(s).toContain("/Count 2 /Kids [3 0 R 6 0 R]");
    expect((s.match(/\/Subtype \/Image/g) ?? []).length).toBe(2);
    // Every xref offset points at its object.
    const xrefAt = Number(/startxref\n(\d+)/.exec(s)![1]);
    expect(s.slice(xrefAt, xrefAt + 4)).toBe("xref");
    const offs = [...s.slice(xrefAt).matchAll(/^(\d{10}) 00000 n $/gm)].map((m) => Number(m[1]));
    expect(offs.length).toBe(8);
    offs.forEach((o, i) => expect(s.slice(o, o + `${i + 1} 0 obj`.length)).toBe(`${i + 1} 0 obj`));
    expect(s).toContain("/Length 7 >>\nstream\n\xff\xd8");
  });
  it("isolates numbers for Arabic pages", () => {
    expect(isolateNumber("-12,00 د.م.")).toBe("\u2066-12,00\u2069 د.م.");
    expect(isolateNumber("79,3 %")).toBe("\u206679,3 %\u2069");
    expect(isolateNumber("1 234,50 د.م.")).toBe("\u20661 234,50\u2069 د.م.");
    expect(isolateNumber("—")).toBe("—");
  });
  it("base64 of big buffers", () => {
    const b = new Uint8Array(100_000).map((_, i) => i % 256);
    expect(atob(toBase64(b)).length).toBe(100_000);
    expect(toBase64(new Uint8Array([104, 105]))).toBe("aGk=");
  });
});
