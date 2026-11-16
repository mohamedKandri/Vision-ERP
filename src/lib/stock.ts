// Stock quantities and food cost, mirroring src-tauri/src/stock.rs.
// Quantities are integer milli-units of the ingredient's unit (150 g = 150000, 1 piece = 1000).
// Ingredient cost is in cents per kg (g), per litre (ml) or per piece.

import { divRound, splitIncluded, type Cents } from "./money";
import type { Names } from "./menu";

export type Unit = "g" | "ml" | "piece";
export const UNITS: Unit[] = ["g", "ml", "piece"];

export interface Ingredient {
  id: number;
  names: Names;
  unit: Unit;
  cost_cents: Cents;
  stock_milli: number;
  min_milli: number;
  active: boolean;
  recipe_count: number;
  low: boolean;
  out: boolean;
  value_cents: Cents;
}

export interface RecipeLine {
  ingredient_id: number;
  qty_milli: number;
}

/** Parse what someone types ("150", "1,5", "0.250") in the ingredient's unit. Max 3 decimals. */
export function parseQty(input: string, allowNegative = false): number | null {
  const s = input.trim().replace(/[\s  ]/g, "").replace(",", ".");
  if (!/^-?\d+(\.\d{0,3})?$/.test(s) && !/^-?\.\d{1,3}$/.test(s)) return null;
  const neg = s.startsWith("-");
  if (neg && !allowNegative) return null;
  const [int, frac = ""] = s.replace("-", "").split(".");
  const v = parseInt(int || "0", 10) * 1000 + parseInt((frac + "000").slice(0, 3), 10);
  if (!Number.isSafeInteger(v)) return null;
  return neg ? -v : v;
}

/** Milli-units back to an editable string in the base unit ("150", "1.5"). */
export function qtyInput(milli: number): string {
  const neg = milli < 0;
  const a = Math.abs(milli);
  const int = Math.floor(a / 1000);
  const frac = String(a % 1000).padStart(3, "0").replace(/0+$/, "");
  return `${neg ? "-" : ""}${int}${frac ? "." + frac : ""}`;
}

const PIECE: Record<string, string> = { fr: "pce", en: "pc", ar: "قطعة" };

export function unitLabel(unit: Unit, lang: string): string {
  return unit === "piece" ? PIECE[lang] ?? "pce" : unit;
}

/** Label of the unit the cost is expressed in: kg, L or piece. */
export function costUnit(unit: Unit, lang: string): string {
  return unit === "g" ? "kg" : unit === "ml" ? "L" : unitLabel(unit, lang);
}

/** "150 g", "2,5 kg", "12 pce". Large weights and volumes switch to kg / L. Latin digits. */
export function formatQty(milli: number, unit: Unit, lang: string): string {
  const nf = (max: number) =>
    new Intl.NumberFormat(`${lang === "ar" ? "ar-MA" : lang === "en" ? "en-MA" : "fr-MA"}-u-nu-latn`, { maximumFractionDigits: max });
  if (unit !== "piece" && Math.abs(milli) >= 1_000_000) {
    return `${nf(3).format(milli / 1_000_000)} ${unit === "g" ? "kg" : "L"}`;
  }
  return `${nf(3).format(milli / 1000)} ${unitLabel(unit, lang)}`;
}

/** Value in milli-cents; same rounding as the Host. */
export function valueMc(unit: Unit, costCents: Cents, milli: number): number {
  const v = milli * costCents;
  return unit === "piece" ? v : divRound(v, 1000);
}

export function mcToCents(mc: number): Cents {
  return divRound(mc, 1000);
}

/** Cost of recipe lines (removals count negative), in cents. Unknown ingredients count 0. */
export function recipeCost(lines: RecipeLine[], ingredients: Map<number, Pick<Ingredient, "unit" | "cost_cents">>): Cents {
  let mc = 0;
  for (const l of lines) {
    const i = ingredients.get(l.ingredient_id);
    if (i) mc += valueMc(i.unit, i.cost_cents, l.qty_milli);
  }
  return mcToCents(mc);
}

export function priceHt(price: Cents, rateBp: number, priceInclude: boolean): Cents {
  return priceInclude ? splitIncluded(price, rateBp).base : price;
}

/** Margin on the price excluding VAT, in basis points. */
export function marginBp(priceHtCents: Cents, cost: Cents): number {
  return priceHtCents > 0 ? divRound((priceHtCents - cost) * 10000, priceHtCents) : 0;
}

/** Food cost as a share of the price excluding VAT, in basis points. */
export function foodCostBp(priceHtCents: Cents, cost: Cents): number {
  return priceHtCents > 0 ? divRound(cost * 10000, priceHtCents) : 0;
}

/** Colour band for a margin: good ≥ 65 %, watch ≥ 50 %, else too low. */
export function marginLevel(bp: number): "good" | "watch" | "low" {
  return bp >= 6500 ? "good" : bp >= 5000 ? "watch" : "low";
}
