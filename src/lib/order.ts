// Shapes of orders as returned by the Host, plus small pure helpers used by the till.

import type { ItemConfig, Names } from "./menu";
import type { Cents, TaxBucket } from "./money";

export type OrderType = "dine_in" | "takeaway" | "delivery";
export type OrderStatus = "open" | "sent" | "preparing" | "ready" | "served" | "closed" | "voided";
export type PaymentStatus = "unpaid" | "partial" | "paid" | "refunded";

export interface OrderLine {
  id: number;
  product_id: number;
  variant_id: number | null;
  config: ItemConfig;
  names: Names;
  details: { fr: string[]; ar: string[]; en: string[] };
  qty: number;
  unit_cents: Cents;
  happy_hour: boolean;
  tax_id: number;
  tax_rate_bp: number;
  price_include: boolean;
  discount_kind: "percent" | "fixed" | "comp" | null;
  discount_value: number | null;
  discount_reason: string | null;
  gross_cents: Cents;
  discount_cents: Cents;
  base_cents: Cents;
  tax_cents: Cents;
  total_cents: Cents;
  note: string;
  station_id: number | null;
  kitchen_status: "new" | "sent" | "done";
  voided: boolean;
  void_reason: string | null;
}

export interface Payment {
  id: number;
  kind: "payment" | "refund";
  method: "cash" | "card";
  amount_cents: Cents;
  tendered_cents: Cents | null;
  change_cents: Cents | null;
  at: string;
}

export interface Order {
  id: number;
  number: number;
  split_index: number;
  parent_id: number | null;
  type: OrderType;
  table_id: number | null;
  table_name: string | null;
  customer_name: string;
  customer_phone: string;
  customer_address: string;
  delivery_fee_cents: Cents;
  note: string;
  status: OrderStatus;
  payment_status: PaymentStatus;
  held: boolean;
  hold_label: string;
  discount_kind: "percent" | "fixed" | null;
  discount_value: number | null;
  discount_reason: string | null;
  subtotal_cents: Cents;
  discount_cents: Cents;
  base_cents: Cents;
  tax_cents: Cents;
  total_cents: Cents;
  paid_cents: Cents;
  due_cents: Cents;
  vat: TaxBucket[];
  register: string;
  created_by_name: string | null;
  created_at: string;
  paid_at: string | null;
  refunded_at: string | null;
  voided_at: string | null;
  lines: OrderLine[];
  payments: Payment[];
}

/** "42" or "42-2" for a split check. */
export function orderLabel(o: { number: number; split_index: number }): string {
  return o.split_index > 0 ? `${o.number}-${o.split_index + 1}` : String(o.number);
}

/** Quick-cash buttons: the exact amount, then the next round note values above it. */
export function quickCash(due: Cents): Cents[] {
  if (due <= 0) return [];
  const notes = [1000, 2000, 5000, 10000, 20000];
  const out = new Set<Cents>([due]);
  for (const step of [500, 1000, 5000, 10000]) out.add(Math.ceil(due / step) * step);
  for (const n of notes) if (n > due) out.add(n);
  return [...out].filter((x) => x >= due).sort((a, b) => a - b).slice(0, 6);
}

/** Cash part of a mixed payment: card first (if any), the rest in cash. */
export function planPayment(due: Cents, cardCents: Cents, tenderedCash: Cents): { ok: boolean; cash: Cents; change: Cents; remaining: Cents } {
  const card = Math.min(Math.max(cardCents, 0), due);
  const cashDue = due - card;
  const cash = Math.min(tenderedCash, cashDue);
  const change = Math.max(0, tenderedCash - cashDue);
  const remaining = cashDue - cash;
  return { ok: remaining === 0, cash, change, remaining };
}
