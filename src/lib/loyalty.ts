// Customers and loyalty (Phase 9). The Host decides; these helpers only display.

import type { Cents } from "./money";

export interface Customer {
  id: number;
  name: string;
  phone: string;
  address: string;
  note: string;
  active: boolean;
  created_at: string;
  orders: number;
  spent_cents: Cents;
  last_order_at: string | null;
  stamps: number;
  points: number;
}

export interface CustomerDetail extends Customer {
  order_list: { id: number; number: number; split_index: number; type: string; status: string; payment_status: string; total_cents: Cents; created_at: string; paid_at: string | null }[];
  moves: { at: string; kind: "earn" | "redeem" | "reverse" | "adjust"; stamps: number; points: number; reason: string | null; number: number | null; split_index: number | null; user: string | null }[];
}

/** What `orders.get` returns about the order's customer (null when no customer). */
export interface OrderLoyalty {
  customer: { id: number; name: string; phone: string };
  mode: "none" | "stamps" | "points";
  stamps: number;
  points: number;
  stamps_needed: number;
  point_value_cents: Cents;
  redeemed_stamps: number;
  redeemed_points: number;
  redeemed: boolean;
  can_redeem: boolean;
  max_points: number;
}

/** Same rule as the Host: digits only, "+212 6…" / "00212 6…" become "06…"; null if too short or long. */
export function normalizePhone(raw: string): string | null {
  const t = raw.trim();
  let d = t.replace(/\D/g, "");
  if (t.startsWith("+") || t.startsWith("00")) {
    if (t.startsWith("00")) d = d.slice(2);
    if (d.startsWith("212") && d.length === 12) d = `0${d.slice(3)}`;
  } else if (d.length === 12 && d.startsWith("212")) {
    d = `0${d.slice(3)}`;
  }
  return d.length >= 6 && d.length <= 16 ? d : null;
}

/** "06 12 34 56 78" for a 10-digit Moroccan number, else unchanged. */
export function formatPhone(p: string): string {
  return /^0\d{9}$/.test(p) ? p.replace(/(\d{2})(?=\d)/g, "$1 ") : p;
}

/** Stamps towards the next reward: 13 stamps with 10 needed -> 1 reward ready + 3 of 10. */
export function stampProgress(stamps: number, needed: number): { rewards: number; current: number } {
  if (needed <= 0) return { rewards: 0, current: 0 };
  return { rewards: Math.floor(Math.max(0, stamps) / needed), current: Math.max(0, stamps) % needed };
}

/** Value of points in cents. */
export function pointsValue(points: number, pointValueCents: Cents): Cents {
  return Math.max(0, points) * Math.max(0, pointValueCents);
}
