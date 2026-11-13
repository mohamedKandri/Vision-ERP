// Turn receipts and X/Z reports into print documents and send them to the Host's queue.

import { translate, type Key } from "../i18n";
import { formatMoney, formatRate } from "./money";
import type { Order } from "./order";
import { buildReceipt, type ReceiptLine } from "./receipt";
import { renderReceiptPng } from "./receiptCanvas";
import { rpc, type Lang, type Settings } from "./rpc";

export interface ReceiptPrinter {
  id: number;
  name: string;
  paper_mm: 58 | 80;
  graphic_mode: boolean;
  drawer: boolean;
  kind: string;
}

export interface CashReportLike {
  register: string;
  z_number: number | null;
  opened_by: string;
  opened_at: string;
  closed_at: string | null;
  float_cents: number;
  cash_sales_cents: number;
  card_sales_cents: number;
  cash_refunds_cents: number;
  card_refunds_cents: number;
  cash_in_cents: number;
  cash_out_cents: number;
  expected_cash_cents: number;
  counted_cash_cents: number | null;
  difference_cents: number | null;
  orders: number;
  average_ticket_cents: number;
  discounts_cents: number;
  comps_cents: number;
  voids: number;
  voids_total_cents: number;
  refunds: number;
  net_sales_cents: number;
  by_type: { type: string; count: number; total_cents: number }[];
  vat: { tax_id: number; rate_bp: number; base: number; tax: number }[];
  movements: { kind: "in" | "out"; amount_cents: number; reason: string; user: string }[];
}

/** X/Z report as printable lines (pure; tested). */
export function reportDoc(r: CashReportLike, shopName: string, lang: Lang, fmtDate: (iso: string) => string): ReceiptLine[] {
  const t = (k: Key, v?: Record<string, string | number>) => translate(lang, k, v);
  const m = (c: number) => formatMoney(c, lang);
  const row = (left: string, right: string, bold = false): ReceiptLine => ({ kind: "row", left, right, bold });
  const out: ReceiptLine[] = [
    { kind: "text", text: shopName, align: "center", bold: true, big: true },
    { kind: "text", text: r.z_number ? t("cash_z_report", { n: r.z_number }) : t("cash_x_report"), align: "center", bold: true, big: true },
    { kind: "text", text: `${t("cash_register")} ${r.register}`, align: "center" },
    { kind: "text", text: t("cash_opened", { u: r.opened_by, d: fmtDate(r.opened_at) }), align: "center" },
  ];
  if (r.closed_at) out.push({ kind: "text", text: `→ ${fmtDate(r.closed_at)}`, align: "center" });
  out.push(
    { kind: "rule" },
    row(t("cash_float"), m(r.float_cents)),
    row(t("cash_cash_sales"), m(r.cash_sales_cents)),
    row(`${t("cash_refunds")} (${t("rc_cash")})`, `-${m(r.cash_refunds_cents)}`),
    row(t("cash_in"), m(r.cash_in_cents)),
    row(t("cash_out"), `-${m(r.cash_out_cents)}`),
    row(t("cash_expected"), m(r.expected_cash_cents), true),
  );
  if (r.counted_cash_cents !== null) out.push(row(t("cash_counted"), m(r.counted_cash_cents), true));
  if (r.difference_cents !== null) out.push(row(t("cash_difference"), m(r.difference_cents), true));
  out.push(
    { kind: "rule" },
    row(t("cash_card_sales"), m(r.card_sales_cents)),
    row(`${t("cash_refunds")} (${r.refunds})`, `-${m(r.cash_refunds_cents + r.card_refunds_cents)}`),
    row(t("cash_net_sales"), m(r.net_sales_cents), true),
    row(t("cash_orders"), String(r.orders)),
    row(t("cash_avg_ticket"), m(r.average_ticket_cents)),
    row(t("cash_discounts"), m(r.discounts_cents)),
    row(t("cash_comps"), m(r.comps_cents)),
    row(`${t("cash_voids")} (${r.voids})`, m(r.voids_total_cents)),
    { kind: "rule" },
    { kind: "text", text: t("cash_by_type"), bold: true },
  );
  for (const b of r.by_type) out.push(row(`${t(`order_type_${b.type}` as Key)} (${b.count})`, m(b.total_cents)));
  out.push({ kind: "text", text: t("rc_vat"), bold: true });
  for (const b of r.vat) out.push(row(`${formatRate(b.rate_bp)} ${t("rc_base")} ${m(b.base)}`, m(b.tax)));
  if (r.movements.length) {
    out.push({ kind: "text", text: t("cash_movements"), bold: true });
    for (const mv of r.movements) out.push(row(`${mv.kind === "in" ? "+" : "-"} ${mv.reason} (${mv.user})`, m(mv.amount_cents)));
  }
  out.push({ kind: "feed" });
  return out;
}

/** Arabic needs shaping the printer can't do, so it goes as an image. */
async function toPrintable(lines: ReceiptLine[], printer: ReceiptPrinter, lang: Lang): Promise<ReceiptLine[]> {
  if (lang !== "ar" && !printer.graphic_mode) return lines;
  const src = await renderReceiptPng(lines, printer.paper_mm === 58 ? 384 : 576, lang === "ar");
  return [{ kind: "raster", src } as unknown as ReceiptLine];
}

export async function printReceipt(o: Order, s: Settings, printer: ReceiptPrinter, opts: { lang: Lang; register: string; reprint?: boolean; kick?: boolean }) {
  const width = printer.paper_mm === 58 ? 32 : 48;
  const lines = buildReceipt(o, s, { lang: opts.lang, width, reprint: opts.reprint });
  const doc = await toPrintable(lines, printer, opts.lang);
  return rpc<{ job_id: number }>("print.receipt", {
    register: opts.register,
    order_id: o.id,
    doc,
    reprint: !!opts.reprint,
    kick: !!opts.kick,
    title: `#${o.number}`,
  });
}

export async function printReport(r: CashReportLike, s: Settings, printer: ReceiptPrinter, lang: Lang, register: string) {
  const lines = reportDoc(r, String(s.shop_name ?? ""), lang, (iso) => new Date(iso).toLocaleString(lang === "ar" ? "ar-MA-u-nu-latn" : lang));
  const doc = await toPrintable(lines, printer, lang);
  return rpc<{ job_id: number }>("print.doc", { register, doc, title: r.z_number ? `Z ${r.z_number}` : "X" });
}
