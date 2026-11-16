// Purchase-order arithmetic, mirroring src-tauri/src/purchasing.rs.
import { divRound, type Cents } from "./money";
import { formatQty, type Unit } from "./stock";

export type PoStatus = "draft" | "ordered" | "partial" | "closed" | "cancelled";

/** Value of `qtyMilli` at `packCost` per pack of `packMilli` (same rounding as the Host). */
export function lineValue(qtyMilli: number, packMilli: number, packCost: Cents): Cents {
  return packMilli > 0 ? divRound(qtyMilli * packCost, packMilli) : 0;
}

/** Still expected on a line, never negative. */
export function remaining(orderedMilli: number, receivedMilli: number): number {
  return Math.max(orderedMilli - receivedMilli, 0);
}

/** "Sachet 5 kg" or, without a label, the pack size itself ("5 kg"). */
export function packName(label: string, packMilli: number, unit: Unit, lang: string): string {
  return label.trim() || formatQty(packMilli, unit, lang);
}

/** Cost per kg / L / piece from a pack price, like the Host's unit_cost. */
export function unitCostOfPack(unit: Unit, packMilli: number, packCost: Cents): Cents {
  if (packMilli <= 0) return 0;
  return divRound(packCost * (unit === "piece" ? 1000 : 1_000_000), packMilli);
}

/** Order total in cents. */
export function poTotal(lines: { qty_packs: number; pack_cost_cents: Cents }[]): Cents {
  return lines.reduce((s, l) => s + l.qty_packs * l.pack_cost_cents, 0);
}
