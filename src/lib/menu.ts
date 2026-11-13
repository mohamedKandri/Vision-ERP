// Pure menu logic: names, happy hour, item configuration (variant + modifiers + combo choices),
// validation against min/max rules, unit price, and the text shown on tickets.

import type { Cents } from "./money";
import type { Lang } from "./rpc";

export type Names = Partial<Record<Lang, string>> & { fr: string };

export interface Station {
  id: number;
  code: string;
  names: Names;
  active: boolean;
}
export interface Category {
  id: number;
  names: Names;
  color: string;
  icon: string;
  sort: number;
  active: boolean;
}
export interface Variant {
  id: number;
  names: Names;
  price_cents: Cents;
  happy_hour_price_cents: Cents | null;
  active: boolean;
}
export interface ModifierOption {
  id: number;
  names: Names;
  kind: "choice" | "extra" | "remove";
  price_cents: Cents;
  max_qty: number;
  is_default: boolean;
  active: boolean;
}
export interface ModifierGroup {
  id: number;
  names: Names;
  min_select: number;
  max_select: number;
  sort: number;
  active: boolean;
  options: ModifierOption[];
}
export interface SlotOption {
  product_id: number;
  variant_id: number | null;
  extra_cents: Cents;
  is_default: boolean;
}
export interface ComboSlot {
  id?: number;
  names: Names;
  options: SlotOption[];
}
export interface Product {
  id: number;
  category_id: number;
  kind: "simple" | "combo";
  names: Names;
  description: Partial<Names>;
  price_cents: Cents;
  happy_hour_price_cents: Cents | null;
  tax_id: number;
  station_id: number | null;
  photo: string;
  active: boolean;
  available: boolean;
  sort: number;
  variants: Variant[];
  group_ids: number[];
  slots: ComboSlot[];
}
export interface Menu {
  stations: Station[];
  categories: Category[];
  products: Product[];
  modifier_groups: ModifierGroup[];
}

export function pickName(names: Partial<Names> | undefined, lang: Lang): string {
  if (!names) return "";
  return names[lang]?.trim() || names.fr || names.en || names.ar || "";
}

/** Is `minutes` (since local midnight) inside the happy-hour window? Windows may cross midnight. */
export function isHappyHour(minutes: number, s: { enabled: boolean; start: number; end: number }): boolean {
  if (!s.enabled || s.start === s.end) return false;
  return s.start < s.end ? minutes >= s.start && minutes < s.end : minutes >= s.start || minutes < s.end;
}

export function minutesOfDay(d: Date): number {
  return d.getHours() * 60 + d.getMinutes();
}

// ------------------------------------------------------------------ configuration

export interface ModSel {
  option_id: number;
  qty: number;
}

/** What the cashier picked for one product (or one part of a combo). */
export interface ItemConfig {
  product_id: number;
  variant_id: number | null;
  modifiers: ModSel[];
  /** Combo only: one entry per slot, same order as product.slots. */
  combo?: ItemConfig[];
}

export class MenuIndex {
  readonly products = new Map<number, Product>();
  readonly groups = new Map<number, ModifierGroup>();
  readonly options = new Map<number, { option: ModifierOption; group: ModifierGroup }>();
  constructor(readonly menu: Menu) {
    for (const p of menu.products) this.products.set(p.id, p);
    for (const g of menu.modifier_groups) {
      this.groups.set(g.id, g);
      for (const o of g.options) this.options.set(o.id, { option: o, group: g });
    }
  }
  product(id: number): Product {
    const p = this.products.get(id);
    if (!p) throw new Error(`unknown product ${id}`);
    return p;
  }
  groupsOf(p: Product): ModifierGroup[] {
    return p.group_ids.map((id) => this.groups.get(id)).filter((g): g is ModifierGroup => !!g && g.active);
  }
}

/** Defaults: first active variant, default modifier options, default combo choices. */
export function defaultConfig(ix: MenuIndex, productId: number, variantId: number | null = null): ItemConfig {
  const p = ix.product(productId);
  const variants = p.variants.filter((v) => v.active);
  const modifiers: ModSel[] = ix
    .groupsOf(p)
    .flatMap((g) => g.options.filter((o) => o.active && o.is_default).map((o) => ({ option_id: o.id, qty: 1 })));
  const cfg: ItemConfig = { product_id: p.id, variant_id: variantId ?? variants[0]?.id ?? null, modifiers };
  if (p.kind === "combo") {
    cfg.combo = p.slots.map((s) => {
      const opt = s.options.find((o) => o.is_default) ?? s.options[0];
      return defaultConfig(ix, opt.product_id, opt.variant_id);
    });
  }
  return cfg;
}

export type ConfigError =
  | { kind: "min"; group_id: number; need: number }
  | { kind: "max"; group_id: number; max: number }
  | { kind: "option_qty"; option_id: number; max: number }
  | { kind: "unknown_option"; option_id: number }
  | { kind: "variant_required"; product_id: number }
  | { kind: "slot_choice"; slot: number }
  | { kind: "unavailable"; product_id: number };

/** Every rule a configuration must satisfy before it can go into the cart. */
export function validateConfig(ix: MenuIndex, cfg: ItemConfig, slotIndex?: number): ConfigError[] {
  const errors: ConfigError[] = [];
  const p = ix.product(cfg.product_id);
  if (!p.available || !p.active) errors.push({ kind: "unavailable", product_id: p.id });
  const variants = p.variants.filter((v) => v.active);
  if (variants.length > 0 && !variants.some((v) => v.id === cfg.variant_id)) errors.push({ kind: "variant_required", product_id: p.id });

  const groups = ix.groupsOf(p);
  const allowed = new Set(groups.flatMap((g) => g.options.filter((o) => o.active).map((o) => o.id)));
  for (const m of cfg.modifiers) {
    if (!allowed.has(m.option_id)) {
      errors.push({ kind: "unknown_option", option_id: m.option_id });
      continue;
    }
    const o = ix.options.get(m.option_id)!.option;
    if (m.qty < 1 || m.qty > o.max_qty || !Number.isInteger(m.qty)) errors.push({ kind: "option_qty", option_id: o.id, max: o.max_qty });
  }
  for (const g of groups) {
    const count = cfg.modifiers.filter((m) => ix.options.get(m.option_id)?.group.id === g.id).reduce((a, m) => a + m.qty, 0);
    if (count < g.min_select) errors.push({ kind: "min", group_id: g.id, need: g.min_select });
    if (count > g.max_select) errors.push({ kind: "max", group_id: g.id, max: g.max_select });
  }

  if (p.kind === "combo") {
    const parts = cfg.combo ?? [];
    p.slots.forEach((s, i) => {
      const part = parts[i];
      const ok = part && s.options.some((o) => o.product_id === part.product_id && (o.variant_id === null || o.variant_id === part.variant_id));
      if (!ok) errors.push({ kind: "slot_choice", slot: i });
      else errors.push(...validateConfig(ix, part, i));
    });
  }
  void slotIndex;
  return errors;
}

function slotOptionFor(p: Product, slot: number, part: ItemConfig): SlotOption | undefined {
  return p.slots[slot]?.options.find(
    (o) => o.product_id === part.product_id && (o.variant_id === null || o.variant_id === part.variant_id),
  );
}

function modifiersTotal(ix: MenuIndex, cfg: ItemConfig): Cents {
  return cfg.modifiers.reduce((a, m) => a + (ix.options.get(m.option_id)?.option.price_cents ?? 0) * m.qty, 0);
}

/** Base price of a product/variant, with the happy-hour price when it applies. */
export function basePrice(p: Product, variantId: number | null, happy: boolean): Cents {
  const v = variantId === null ? undefined : p.variants.find((x) => x.id === variantId);
  const src = v ?? p;
  return happy && src.happy_hour_price_cents !== null ? src.happy_hour_price_cents : src.price_cents;
}

/**
 * Unit price of a configured item.
 * - simple: base (variant / happy hour) + modifiers.
 * - combo: combo price (happy hour) + combo modifiers (e.g. Maxi) + each slot's extra
 *   + modifiers chosen on the parts (extra cheese in the combo's burger is paid).
 */
export function unitPrice(ix: MenuIndex, cfg: ItemConfig, happy = false): Cents {
  const p = ix.product(cfg.product_id);
  if (p.kind !== "combo") return basePrice(p, cfg.variant_id, happy) + modifiersTotal(ix, cfg);
  let total = basePrice(p, null, happy) + modifiersTotal(ix, cfg);
  (cfg.combo ?? []).forEach((part, i) => {
    total += (slotOptionFor(p, i, part)?.extra_cents ?? 0) + modifiersTotal(ix, part);
  });
  return total;
}

/** Human-readable detail lines for cart, receipt and kitchen ticket. */
export function describe(ix: MenuIndex, cfg: ItemConfig, lang: Lang): string[] {
  const p = ix.product(cfg.product_id);
  const lines: string[] = [];
  const v = p.variants.find((x) => x.id === cfg.variant_id);
  if (v) lines.push(pickName(v.names, lang));
  for (const m of cfg.modifiers) {
    const hit = ix.options.get(m.option_id);
    if (!hit) continue;
    const name = pickName(hit.option.names, lang);
    const prefix = hit.option.kind === "extra" ? "+ " : hit.option.kind === "remove" ? "− " : "";
    lines.push(`${prefix}${name}${m.qty > 1 ? ` ×${m.qty}` : ""}`);
  }
  (cfg.combo ?? []).forEach((part) => {
    const sub = ix.product(part.product_id);
    const detail = describe(ix, part, lang);
    lines.push(`• ${pickName(sub.names, lang)}${detail.length ? ` (${detail.join(", ")})` : ""}`);
  });
  return lines;
}

/** Change the quantity of one modifier option, keeping within the option's max_qty. Single-choice groups swap. */
export function toggleModifier(ix: MenuIndex, cfg: ItemConfig, optionId: number, delta: 1 | -1): ItemConfig {
  const hit = ix.options.get(optionId);
  if (!hit) return cfg;
  const { option, group } = hit;
  let mods = cfg.modifiers.map((m) => ({ ...m }));
  const cur = mods.find((m) => m.option_id === optionId);
  if (delta > 0) {
    if (group.max_select === 1) {
      // Radio behaviour: replace whatever is selected in this group.
      mods = mods.filter((m) => ix.options.get(m.option_id)?.group.id !== group.id);
      mods.push({ option_id: optionId, qty: 1 });
    } else {
      const inGroup = mods.filter((m) => ix.options.get(m.option_id)?.group.id === group.id).reduce((a, m) => a + m.qty, 0);
      if (inGroup >= group.max_select) return cfg;
      if (cur) {
        if (cur.qty >= option.max_qty) return cfg;
        cur.qty += 1;
      } else mods.push({ option_id: optionId, qty: 1 });
    }
  } else if (cur) {
    cur.qty -= 1;
    if (cur.qty <= 0) mods = mods.filter((m) => m.option_id !== optionId);
  }
  return { ...cfg, modifiers: mods };
}
