// Receipt layout as a list of printer-agnostic lines. The on-screen preview renders it now;
// Phase 4 turns the same list into ESC/POS bytes, so what you see is what prints.

import { translate, type Key } from "../i18n";
import { pickName } from "./menu";
import { formatMoney, formatRate, percentOf } from "./money";
import { orderLabel, type Order } from "./order";
import type { Lang, Settings } from "./rpc";

export type Align = "left" | "center" | "right";
export type ReceiptLine =
  | { kind: "logo"; src: string }
  | { kind: "text"; text: string; align?: Align; bold?: boolean; big?: boolean }
  | { kind: "row"; left: string; right: string; bold?: boolean; big?: boolean }
  | { kind: "rule" }
  | { kind: "qr"; data: string }
  | { kind: "feed" };

export interface ReceiptOpts {
  lang: Lang;
  /** Characters per line: 32 for 58 mm paper, 48 for 80 mm. */
  width: 32 | 48;
  reprint?: boolean;
  /** Local time formatter; injectable for tests. */
  formatDate?: (iso: string) => string;
}

/** Split text into lines of at most `width` characters, breaking on spaces when possible. */
export function wrap(text: string, width: number): string[] {
  const out: string[] = [];
  for (const para of text.split("\n")) {
    let line = "";
    for (const word of para.split(/\s+/).filter(Boolean)) {
      if (!line) line = word;
      else if (line.length + 1 + word.length <= width) line += " " + word;
      else {
        out.push(line);
        line = word;
      }
      while (line.length > width) {
        out.push(line.slice(0, width));
        line = line.slice(width);
      }
    }
    out.push(line);
  }
  return out;
}

/** Left text + right-aligned amount on one line; the left side wraps if it doesn't fit. */
export function row(left: string, right: string, width: number): string[] {
  const room = width - right.length - 1;
  const parts = wrap(left, Math.max(room, 8));
  const last = parts.pop() ?? "";
  const lines = parts;
  if (last.length + 1 + right.length <= width) lines.push(last + " ".repeat(width - last.length - right.length) + right);
  else lines.push(last, " ".repeat(Math.max(0, width - right.length)) + right);
  return lines;
}

export function buildReceipt(o: Order, s: Settings, opts: ReceiptOpts): ReceiptLine[] {
  const L = opts.lang;
  const t = (k: Key, v?: Record<string, string | number>) => translate(L, k, v);
  const m = (c: number) => formatMoney(c, L);
  const fmtDate = opts.formatDate ?? ((iso: string) => new Date(iso).toLocaleString(L === "ar" ? "ar-MA-u-nu-latn" : L === "en" ? "en-GB" : "fr-FR"));
  const out: ReceiptLine[] = [];
  const text = (x: string, more: Partial<Extract<ReceiptLine, { kind: "text" }>> = {}) => {
    if (x.trim()) out.push({ kind: "text", text: x, ...more });
  };

  if (s.shop_logo) out.push({ kind: "logo", src: String(s.shop_logo) });
  text(String(s.shop_name ?? ""), { align: "center", bold: true, big: true });
  for (const l of String(s.shop_address ?? "").split("\n")) text(l, { align: "center" });
  if (s.shop_phone) text(`${t("rc_tel")} ${s.shop_phone}`, { align: "center" });
  const ids = [
    s.shop_ice ? `ICE ${s.shop_ice}` : "",
    s.shop_if ? `IF ${s.shop_if}` : "",
    s.shop_rc ? `RC ${s.shop_rc}` : "",
    s.shop_patente ? `Patente ${s.shop_patente}` : "",
  ].filter(Boolean);
  for (const l of ids) text(l, { align: "center" });
  for (const l of String(s.receipt_header ?? "").split("\n")) text(l, { align: "center" });
  if (opts.reprint) text(`*** ${t("rc_reprint")} ***`, { align: "center", bold: true });
  if (o.payment_status === "refunded") text(`*** ${t("rc_refunded")} ***`, { align: "center", bold: true });
  if (o.status === "voided") text(`*** ${t("rc_voided")} ***`, { align: "center", bold: true });
  out.push({ kind: "rule" });

  text(`${t("rc_order")} #${orderLabel(o)}`, { align: "center", bold: true, big: true });
  const typeLabel =
    o.type === "dine_in"
      ? `${t("order_type_dine_in")}${o.table_name ? ` · ${t("rc_table")} ${o.table_name}` : ""}`
      : o.type === "takeaway"
        ? `${t("order_type_takeaway")}${o.customer_name ? ` · ${o.customer_name}` : ""}`
        : t("order_type_delivery");
  text(typeLabel, { align: "center" });
  if (o.type === "delivery") {
    text(o.customer_name);
    text(o.customer_phone);
    for (const l of wrap(o.customer_address, opts.width)) text(l);
  }
  text(fmtDate(o.created_at), { align: "center" });
  if (o.created_by_name) text(`${t("rc_cashier")}: ${o.created_by_name}`, { align: "center" });
  out.push({ kind: "rule" });

  for (const l of o.lines.filter((x) => !x.voided)) {
    out.push({ kind: "row", left: `${l.qty} x ${pickName(l.names, L)}`, right: m(l.gross_cents), bold: true });
    for (const d of l.details[L] ?? l.details.fr ?? []) text(`   ${d}`);
    if (l.note) text(`   ${t("rc_note")}: ${l.note}`);
    if (l.discount_kind === "comp") out.push({ kind: "row", left: `   ${t("rc_comp")}${l.discount_reason ? ` (${l.discount_reason})` : ""}`, right: `-${m(l.gross_cents)}` });
    else if (l.discount_kind === "percent")
      out.push({ kind: "row", left: `   ${t("rc_discount")} ${formatRate(l.discount_value ?? 0)}`, right: `-${m(percentOf(l.gross_cents, l.discount_value ?? 0))}` });
    else if (l.discount_kind === "fixed") out.push({ kind: "row", left: `   ${t("rc_discount")}`, right: `-${m(Math.min(l.discount_value ?? 0, l.gross_cents))}` });
  }
  out.push({ kind: "rule" });

  const linesTotal = o.subtotal_cents - o.delivery_fee_cents;
  out.push({ kind: "row", left: t("rc_subtotal"), right: m(linesTotal) });
  if (o.discount_kind) {
    const off = linesTotal + o.delivery_fee_cents - o.total_cents;
    out.push({ kind: "row", left: `${t("rc_discount")}${o.discount_kind === "percent" ? ` ${formatRate(o.discount_value ?? 0)}` : ""}`, right: `-${m(off)}` });
  }
  if (o.delivery_fee_cents > 0) out.push({ kind: "row", left: t("rc_delivery_fee"), right: m(o.delivery_fee_cents) });
  out.push({ kind: "row", left: t("rc_total"), right: m(o.total_cents), bold: true, big: true });
  out.push({ kind: "feed" });

  for (const b of o.vat) {
    // Always "TVA 10 %": the tax's own name (e.g. "TVA 10% TTC") would read oddly next to "HT".
    const name = `${t("rc_vat")} ${formatRate(b.rate_bp)}`;
    out.push({ kind: "row", left: `${name} ${t("rc_base")} ${m(b.base)}`, right: m(b.tax) });
  }
  out.push({ kind: "rule" });

  for (const p of o.payments) {
    const label = p.kind === "refund" ? `${t("rc_refund")} ${t(p.method === "cash" ? "rc_cash" : "rc_card")}` : t(p.method === "cash" ? "rc_cash" : "rc_card");
    if (p.method === "cash" && p.tendered_cents && p.tendered_cents !== p.amount_cents) {
      out.push({ kind: "row", left: `${label} (${t("rc_tendered")})`, right: m(p.tendered_cents) });
      out.push({ kind: "row", left: t("rc_change"), right: m(p.change_cents ?? 0) });
    } else out.push({ kind: "row", left: label, right: m(p.amount_cents) });
  }
  if (o.due_cents > 0 && o.status !== "voided") out.push({ kind: "row", left: t("rc_due"), right: m(o.due_cents), bold: true });

  if (s.receipt_footer) {
    out.push({ kind: "feed" });
    for (const l of String(s.receipt_footer).split("\n")) text(l, { align: "center" });
  }
  if (s.receipt_show_qr) out.push({ kind: "qr", data: `BURGEROS|${orderLabel(o)}|${o.created_at}|${o.total_cents}` });
  out.push({ kind: "feed" });
  return out;
}

/** Plain-text rendering at a fixed width; used by tests and by the Phase 4 text printer. */
export function receiptText(lines: ReceiptLine[], width: number): string[] {
  const out: string[] = [];
  for (const l of lines) {
    if (l.kind === "rule") out.push("-".repeat(width));
    else if (l.kind === "feed") out.push("");
    else if (l.kind === "row") out.push(...row(l.left, l.right, width));
    else if (l.kind === "text")
      for (const w of wrap(l.text, width)) {
        const pad = width - w.length;
        out.push(l.align === "center" ? " ".repeat(Math.floor(pad / 2)) + w : l.align === "right" ? " ".repeat(pad) + w : w);
      }
  }
  return out;
}
