// Reports (Phase 8): date ranges, the Host's report shape, and the tables shown on screen,
// exported to CSV and drawn into the PDF. One table model, three outputs, same numbers.

import type { Names } from "./menu";
import { pickName } from "./menu";
import { formatMoney, formatRate } from "./money";
import type { Lang } from "./rpc";
import { formatQty, type Unit } from "./stock";

// ------------------------------------------------------------------ date ranges

export interface RangeIn {
  from: string; // UTC instant, included
  to: string; // UTC instant, excluded
  tz_offset_min: number;
}

/** Local midnight of the day containing `d`. */
export function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}
export function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}
/** "2026-11-17" (local) */
export function dayKey(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
/** Local date from "YYYY-MM-DD", or null. */
export function parseDay(s: string): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return null;
  const d = new Date(+m[1], +m[2] - 1, +m[3]);
  return dayKey(d) === s ? d : null;
}
/** Local days [first, last] inclusive, as UTC instants for the Host. */
export function dayRange(first: Date, last: Date): RangeIn {
  const from = startOfDay(first);
  const to = addDays(startOfDay(last), 1);
  return { from: from.toISOString(), to: to.toISOString(), tz_offset_min: -from.getTimezoneOffset() };
}

export type Preset = "today" | "yesterday" | "week" | "last7" | "month" | "last_month" | "last30";
export const PRESETS: Preset[] = ["today", "yesterday", "week", "last7", "month", "last_month", "last30"];

/** First and last local day of a preset. Weeks start on Monday. */
export function presetDays(p: Preset, now: Date): [Date, Date] {
  const today = startOfDay(now);
  switch (p) {
    case "today":
      return [today, today];
    case "yesterday":
      return [addDays(today, -1), addDays(today, -1)];
    case "week":
      return [addDays(today, -((today.getDay() + 6) % 7)), today];
    case "last7":
      return [addDays(today, -6), today];
    case "last30":
      return [addDays(today, -29), today];
    case "month":
      return [new Date(today.getFullYear(), today.getMonth(), 1), today];
    case "last_month":
      return [new Date(today.getFullYear(), today.getMonth() - 1, 1), new Date(today.getFullYear(), today.getMonth(), 0)];
  }
}

// ------------------------------------------------------------------ the Host's report

type N = Partial<Names> | null;
export interface Report {
  range: RangeIn;
  totals: {
    orders: number; refunds: number; gross_cents: number; discount_cents: number; base_cents: number; tax_cents: number;
    net_cents: number; sales_cents: number; refunded_cents: number; delivery_fees_cents: number; avg_ticket_cents: number;
  };
  by_hour: { hour: number; orders: number; total_cents: number }[];
  by_day: { day: string; orders: number; total_cents: number }[];
  by_product: { product_id: number; names: N; category_id: number | null; kind: string | null; qty: number; total_cents: number; base_cents: number; discount_cents: number; cost_cents: number; margin_cents: number; food_cost_bp: number | null }[];
  by_category: { category_id: number; names: N; qty: number; total_cents: number; base_cents: number; cost_cents: number; margin_cents: number; food_cost_bp: number | null }[];
  by_type: { type: string; orders: number; total_cents: number; avg_ticket_cents: number }[];
  by_staff: { user_id: number; name: string; orders: number; total_cents: number; avg_ticket_cents: number; discount_cents: number }[];
  by_payment: { method: string; count: number; paid_cents: number; refunded_cents: number; net_cents: number }[];
  vat: { tax_id: number; name: string | null; rate_bp: number; base_cents: number; tax_cents: number; total_cents: number }[];
  food_cost: { revenue_ht_cents: number; cost_cents: number; margin_cents: number; food_cost_bp: number | null };
  discounts: { count: number; total_cents: number; comp_cents: number; items: { number: number; split_index: number; at: string; target: "line" | "order"; names: N; qty: number | null; kind: string; value: number | null; amount_cents: number; reason: string | null; by: string | null }[] };
  voids: {
    orders_cents: number; lines_cents: number; refunds_cents: number;
    orders: { number: number; split_index: number; at: string; amount_cents: number; reason: string | null; by: string | null; created_by: string }[];
    lines: { number: number; split_index: number; at: string; names: N; qty: number; amount_cents: number; reason: string | null; by: string | null }[];
    refunds: { number: number; split_index: number; at: string; amount_cents: number; reason: string | null; by: string | null }[];
  };
  stock: { value_cents: number; ingredients: number; moves: Record<string, number> };
  waste: {
    total_cents: number;
    by_ingredient: { ingredient_id: number; names: N; unit: Unit; qty_milli: number; value_cents: number }[];
    items: { at: string; reason: string | null; product_names: N; user: string | null; ingredients: number; value_cents: number; ingredient_names: N; unit: Unit | null; qty_milli: number }[];
  };
}

// ------------------------------------------------------------------ tables

export type Cell =
  | { text: string }
  | { cents: number }
  | { int: number }
  | { bp: number | null }
  | { milli: number; unit: Unit };

export interface Table {
  id: string;
  title: string;
  columns: { label: string; num?: boolean }[];
  rows: Cell[][];
  total?: Cell[];
  note?: string;
}

export const isNum = (c: Cell) => !("text" in c);

/** On-screen / PDF text of a cell. */
export function cellText(c: Cell, lang: Lang): string {
  if ("text" in c) return c.text;
  if ("cents" in c) return formatMoney(c.cents || 0, lang); // never "-0,00"
  if ("int" in c) return new Intl.NumberFormat("fr-MA-u-nu-latn").format(c.int).replace(/ | /g, " ");
  if ("bp" in c) return c.bp === null ? "—" : formatPercent(c.bp, lang);
  return formatQty(c.milli, c.unit, lang);
}

/** "45,7 %" (fr/ar) or "45.7 %" (en), one decimal at most, Latin digits. */
export function formatPercent(bp: number, lang: Lang): string {
  const nf = new Intl.NumberFormat(`${lang === "en" ? "en" : "fr"}-MA-u-nu-latn`, { maximumFractionDigits: 1 });
  return `${nf.format(bp / 100)} %`;
}

type T = (key: string, vars?: Record<string, string | number>) => string;
const txt = (s: string | null | undefined): Cell => ({ text: s ?? "" });
const ticket = (n: number, split: number) => (split ? `#${n}-${split}` : `#${n}`);

/** "10 %", "5,00 DH", "Offert" */
function discountLabel(kind: string, value: number | null, t: T, lang: Lang): string {
  if (kind === "percent") return formatRate(value ?? 0);
  if (kind === "fixed") return formatMoney(value ?? 0, lang);
  return t("rp_comp");
}

/** Every table of a report, in the order they are shown and exported. */
export function reportTables(r: Report, t: T, lang: Lang, fmtTime: (iso: string) => string, catName: (id: number) => string = () => ""): Table[] {
  const name = (n: N) => pickName(n ?? undefined, lang);
  const tt = r.totals;
  const out: Table[] = [];
  out.push({
    id: "summary",
    title: t("rp_summary"),
    columns: [{ label: "" }, { label: t("rp_amount"), num: true }],
    rows: [
      [txt(t("rp_orders")), { int: tt.orders }],
      [txt(t("rp_gross")), { cents: tt.gross_cents }],
      [txt(t("rp_discounts")), { cents: -tt.discount_cents }],
      [txt(t("rp_refunds_n", { n: tt.refunds })), { cents: -tt.refunded_cents }],
      [txt(t("rp_net")), { cents: tt.net_cents }],
      [txt(t("rp_net_ht")), { cents: tt.base_cents }],
      [txt(t("rp_vat")), { cents: tt.tax_cents }],
      [txt(t("rp_delivery_fees")), { cents: tt.delivery_fees_cents }],
      [txt(t("rp_avg_ticket")), { cents: tt.avg_ticket_cents }],
    ],
    note: t("rp_summary_note"),
  });
  if (r.by_day.length > 1) {
    out.push({
      id: "by_day", title: t("rp_by_day"),
      columns: [{ label: t("rp_day") }, { label: t("rp_orders"), num: true }, { label: t("rp_net"), num: true }],
      rows: r.by_day.map((d) => [txt(d.day), { int: d.orders }, { cents: d.total_cents }]),
      total: [txt(t("rp_total")), { int: tt.orders }, { cents: tt.net_cents }],
    });
  }
  const hours = r.by_hour.filter((h) => h.orders || h.total_cents);
  out.push({
    id: "by_hour", title: t("rp_by_hour"),
    columns: [{ label: t("rp_hour") }, { label: t("rp_orders"), num: true }, { label: t("rp_net"), num: true }],
    rows: hours.map((h) => [txt(`${String(h.hour).padStart(2, "0")}:00`), { int: h.orders }, { cents: h.total_cents }]),
  });
  out.push({
    id: "by_product", title: t("rp_by_product"),
    columns: [{ label: t("rp_product") }, { label: t("rp_qty"), num: true }, { label: t("rp_net"), num: true }, { label: t("rp_net_ht"), num: true },
      { label: t("rp_cost"), num: true }, { label: t("rp_margin"), num: true }, { label: t("rp_food_cost"), num: true }],
    rows: r.by_product.map((p) => [txt(name(p.names)), { int: p.qty }, { cents: p.total_cents }, { cents: p.base_cents }, { cents: p.cost_cents }, { cents: p.margin_cents }, { bp: p.food_cost_bp }]),
    total: [txt(t("rp_total")), { int: r.by_product.reduce((s, p) => s + p.qty, 0) }, { cents: r.by_product.reduce((s, p) => s + p.total_cents, 0) },
      { cents: r.food_cost.revenue_ht_cents }, { cents: r.food_cost.cost_cents }, { cents: r.food_cost.margin_cents }, { bp: r.food_cost.food_cost_bp }],
    note: t("rp_product_note"),
  });
  out.push({
    id: "by_category", title: t("rp_by_category"),
    columns: [{ label: t("rp_category") }, { label: t("rp_qty"), num: true }, { label: t("rp_net"), num: true }, { label: t("rp_cost"), num: true }, { label: t("rp_food_cost"), num: true }],
    rows: r.by_category.map((c) => [txt(name(c.names) || catName(c.category_id)), { int: c.qty }, { cents: c.total_cents }, { cents: c.cost_cents }, { bp: c.food_cost_bp }]),
  });
  out.push({
    id: "by_type", title: t("rp_by_type"),
    columns: [{ label: t("rp_type") }, { label: t("rp_orders"), num: true }, { label: t("rp_net"), num: true }, { label: t("rp_avg_ticket"), num: true }],
    rows: r.by_type.map((x) => [txt(t(`order_type_${x.type}`)), { int: x.orders }, { cents: x.total_cents }, { cents: x.avg_ticket_cents }]),
  });
  out.push({
    id: "by_payment", title: t("rp_by_payment"),
    columns: [{ label: t("rp_method") }, { label: t("rp_payments"), num: true }, { label: t("rp_paid"), num: true }, { label: t("rp_refunded"), num: true }, { label: t("rp_net"), num: true }],
    rows: r.by_payment.map((p) => [txt(t(p.method === "cash" ? "pos_cash" : "pos_card")), { int: p.count }, { cents: p.paid_cents }, { cents: -p.refunded_cents }, { cents: p.net_cents }]),
    total: [txt(t("rp_total")), { int: r.by_payment.reduce((s, p) => s + p.count, 0) }, { cents: r.by_payment.reduce((s, p) => s + p.paid_cents, 0) },
      { cents: -r.by_payment.reduce((s, p) => s + p.refunded_cents, 0) }, { cents: r.by_payment.reduce((s, p) => s + p.net_cents, 0) }],
    note: t("rp_payment_note"),
  });
  out.push({
    id: "by_staff", title: t("rp_by_staff"),
    columns: [{ label: t("rp_staff") }, { label: t("rp_orders"), num: true }, { label: t("rp_net"), num: true }, { label: t("rp_avg_ticket"), num: true }, { label: t("rp_discounts"), num: true }],
    rows: r.by_staff.map((s) => [txt(s.name), { int: s.orders }, { cents: s.total_cents }, { cents: s.avg_ticket_cents }, { cents: s.discount_cents }]),
  });
  out.push({
    id: "vat", title: t("rp_vat_report"),
    columns: [{ label: t("rp_tax") }, { label: t("rp_rate"), num: true }, { label: t("rp_base_ht"), num: true }, { label: t("rp_vat"), num: true }, { label: t("rp_ttc"), num: true }],
    rows: r.vat.map((v) => [txt(v.name ?? ""), { bp: v.rate_bp }, { cents: v.base_cents }, { cents: v.tax_cents }, { cents: v.total_cents }]),
    total: [txt(t("rp_total")), txt(""), { cents: r.vat.reduce((s, v) => s + v.base_cents, 0) }, { cents: r.vat.reduce((s, v) => s + v.tax_cents, 0) }, { cents: r.vat.reduce((s, v) => s + v.total_cents, 0) }],
    note: t("rp_vat_note"),
  });
  out.push({
    id: "discounts", title: t("rp_discounts_report"),
    columns: [{ label: t("rp_when") }, { label: t("rp_ticket") }, { label: t("rp_item") }, { label: t("rp_discount") }, { label: t("rp_reason") }, { label: t("rp_by") }, { label: t("rp_amount"), num: true }],
    rows: r.discounts.items.map((d) => [txt(fmtTime(d.at)), txt(ticket(d.number, d.split_index)),
      txt(d.target === "order" ? t("rp_whole_order") : `${d.qty ?? 1} × ${name(d.names)}`), txt(discountLabel(d.kind, d.value, t, lang)),
      txt(d.reason), txt(d.by), { cents: d.amount_cents }]),
    total: [txt(t("rp_total")), txt(""), txt(""), txt(""), txt(t("rp_comps_incl", { v: formatMoney(r.discounts.comp_cents, lang) })), txt(""), { cents: r.discounts.total_cents }],
  });
  const v = r.voids;
  out.push({
    id: "voids", title: t("rp_voids_report"),
    columns: [{ label: t("rp_when") }, { label: t("rp_ticket") }, { label: t("rp_what") }, { label: t("rp_reason") }, { label: t("rp_by") }, { label: t("rp_amount"), num: true }],
    rows: [
      ...v.orders.map((o) => [o.at, [txt(fmtTime(o.at)), txt(ticket(o.number, o.split_index)), txt(t("rp_void_order")), txt(o.reason), txt(o.by), { cents: o.amount_cents }]] as [string, Cell[]]),
      ...v.lines.map((l) => [l.at, [txt(fmtTime(l.at)), txt(ticket(l.number, l.split_index)), txt(`${t("rp_void_line")}: ${l.qty} × ${name(l.names)}`), txt(l.reason), txt(l.by), { cents: l.amount_cents }]] as [string, Cell[]]),
      ...v.refunds.map((o) => [o.at, [txt(fmtTime(o.at)), txt(ticket(o.number, o.split_index)), txt(t("rp_refund")), txt(o.reason), txt(o.by), { cents: o.amount_cents }]] as [string, Cell[]]),
    ].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0)).map(([, row]) => row),
    total: [txt(t("rp_total")), txt(""), txt(""), txt(""), txt(""), { cents: v.orders_cents + v.lines_cents + v.refunds_cents }],
    note: t("rp_voids_note"),
  });
  const fc = r.food_cost;
  out.push({
    id: "food_cost", title: t("rp_food_cost_report"),
    columns: [{ label: "" }, { label: t("rp_amount"), num: true }],
    rows: [
      [txt(t("rp_revenue_ht")), { cents: fc.revenue_ht_cents }],
      [txt(t("rp_cost")), { cents: fc.cost_cents }],
      [txt(t("rp_margin")), { cents: fc.margin_cents }],
      [txt(t("rp_food_cost")), { bp: fc.food_cost_bp }],
    ],
    note: t("rp_food_cost_note"),
  });
  const m = r.stock.moves;
  out.push({
    id: "stock", title: t("rp_stock_report"),
    columns: [{ label: "" }, { label: t("rp_amount"), num: true }],
    rows: [
      [txt(t("rp_stock_value_now", { n: r.stock.ingredients })), { cents: r.stock.value_cents }],
      [txt(t("rp_stock_purchases")), { cents: m.purchase ?? 0 }],
      [txt(t("rp_stock_sales")), { cents: (m.sale ?? 0) + (m.return ?? 0) }],
      [txt(t("rp_stock_waste")), { cents: m.waste ?? 0 }],
      [txt(t("rp_stock_adjust")), { cents: (m.adjust ?? 0) + (m.count ?? 0) + (m.initial ?? 0) }],
    ],
    note: t("rp_stock_note"),
  });
  out.push({
    id: "waste", title: t("rp_waste_report"),
    columns: [{ label: t("rp_ingredient") }, { label: t("rp_qty"), num: true }, { label: t("rp_value"), num: true }],
    rows: r.waste.by_ingredient.map((w) => [txt(name(w.names)), { milli: w.qty_milli, unit: w.unit }, { cents: w.value_cents }]),
    total: [txt(t("rp_total")), txt(""), { cents: r.waste.total_cents }],
  });
  out.push({
    id: "waste_items", title: t("rp_waste_items"),
    columns: [{ label: t("rp_when") }, { label: t("rp_what") }, { label: t("rp_reason") }, { label: t("rp_by") }, { label: t("rp_value"), num: true }],
    rows: r.waste.items.map((w) => [txt(fmtTime(w.at)),
      txt(w.product_names ? name(w.product_names) : w.ingredient_names && w.unit ? `${formatQty(w.qty_milli, w.unit, lang)} ${name(w.ingredient_names)}` : t("rp_n_ingredients", { n: w.ingredients })),
      txt(w.reason), txt(w.user), { cents: w.value_cents }]),
  });
  return out;
}

// ------------------------------------------------------------------ CSV

/** French and Arabic Excel expect ";" and a decimal comma; English "," and a point. */
export function csvFormat(lang: Lang): { sep: string; dec: string } {
  return lang === "en" ? { sep: ",", dec: "." } : { sep: ";", dec: "," };
}

/** Raw CSV value: numbers without currency or thousands separators, text protected from formulas. */
export function csvValue(c: Cell, lang: Lang): string {
  const { dec } = csvFormat(lang);
  const fixed = (n: number, digits: number) => n.toFixed(digits).replace(".", dec);
  if ("text" in c) return /^[=+\-@\t\r]/.test(c.text) ? `'${c.text}` : c.text;
  if ("cents" in c) return fixed((c.cents || 0) / 100, 2);
  if ("int" in c) return String(c.int);
  if ("bp" in c) return c.bp === null ? "" : fixed(c.bp / 100, 2);
  return fixed(c.unit === "piece" ? c.milli / 1000 : c.milli / 1_000_000, 3); // pieces, kg or L
}

export function csvEscape(v: string, sep: string): string {
  return v.includes(sep) || /["\r\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v;
}

/** One file: a title block, then each table (title row, header, rows, total), a blank line between. */
export function toCsv(head: string[], tables: Table[], lang: Lang): string {
  const { sep } = csvFormat(lang);
  const row = (cells: string[]) => cells.map((c) => csvEscape(c, sep)).join(sep);
  const lines: string[] = head.map((h) => row([h]));
  for (const tb of tables) {
    lines.push("", row([tb.title]), row(tb.columns.map((c) => c.label)));
    for (const r of tb.rows) lines.push(row(r.map((c) => csvValue(c, lang))));
    if (tb.total) lines.push(row(tb.total.map((c) => csvValue(c, lang))));
  }
  return "﻿" + lines.join("\r\n") + "\r\n"; // BOM: Excel opens UTF-8 (Arabic) correctly
}

/** "ventes_2026-11-01_2026-11-17.csv" */
export function exportName(prefix: string, range: RangeIn, ext: "csv" | "pdf"): string {
  const first = dayKey(new Date(range.from));
  const last = dayKey(new Date(new Date(range.to).getTime() - 1));
  return `${prefix}_${first}${first === last ? "" : `_${last}`}.${ext}`;
}

/** Base64 of bytes, in chunks (no call-stack limit on big files). */
export function toBase64(bytes: Uint8Array): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}
