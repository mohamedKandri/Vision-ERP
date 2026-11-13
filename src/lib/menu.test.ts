import { describe, expect, it } from "vitest";
import {
  basePrice,
  defaultConfig,
  describe as describeItem,
  isHappyHour,
  MenuIndex,
  pickName,
  toggleModifier,
  unitPrice,
  validateConfig,
  type Menu,
  type ModifierGroup,
  type Product,
} from "./menu";

const n = (fr: string, ar?: string, en?: string) => ({ fr, ...(ar ? { ar } : {}), ...(en ? { en } : {}) });

const groups: ModifierGroup[] = [
  { id: 1, names: n("Cuisson"), min_select: 1, max_select: 1, sort: 0, active: true, options: [
    { id: 11, names: n("Saignant"), kind: "choice", price_cents: 0, max_qty: 1, is_default: false, active: true },
    { id: 12, names: n("À point"), kind: "choice", price_cents: 0, max_qty: 1, is_default: true, active: true },
  ] },
  { id: 2, names: n("Suppléments"), min_select: 0, max_select: 3, sort: 0, active: true, options: [
    { id: 21, names: n("Fromage", "جبن", "Cheese"), kind: "extra", price_cents: 500, max_qty: 2, is_default: false, active: true },
    { id: 22, names: n("Bacon"), kind: "extra", price_cents: 800, max_qty: 1, is_default: false, active: true },
    { id: 23, names: n("Old"), kind: "extra", price_cents: 100, max_qty: 1, is_default: false, active: false },
  ] },
  { id: 3, names: n("Retirer"), min_select: 0, max_select: 5, sort: 0, active: true, options: [
    { id: 31, names: n("Sans oignon"), kind: "remove", price_cents: 0, max_qty: 1, is_default: false, active: true },
  ] },
  { id: 4, names: n("Taille"), min_select: 1, max_select: 1, sort: 0, active: true, options: [
    { id: 41, names: n("Normal"), kind: "choice", price_cents: 0, max_qty: 1, is_default: true, active: true },
    { id: 42, names: n("Maxi"), kind: "choice", price_cents: 1000, max_qty: 1, is_default: false, active: true },
  ] },
];

const base = { category_id: 1, description: {}, tax_id: 1, station_id: 1, photo: "", active: true, available: true, sort: 0, slots: [] };
const products: Product[] = [
  { ...base, id: 1, kind: "simple", names: n("Classic", "كلاسيك"), price_cents: 4500, happy_hour_price_cents: null, group_ids: [1, 2, 3],
    variants: [
      { id: 101, names: n("Simple"), price_cents: 4500, happy_hour_price_cents: 3900, active: true },
      { id: 102, names: n("Double"), price_cents: 6000, happy_hour_price_cents: null, active: true },
      { id: 103, names: n("Old"), price_cents: 1, happy_hour_price_cents: null, active: false },
    ] },
  { ...base, id: 2, kind: "simple", names: n("Frites"), price_cents: 1500, happy_hour_price_cents: 1000, group_ids: [], variants: [] },
  { ...base, id: 3, kind: "simple", names: n("Potatoes"), price_cents: 1800, happy_hour_price_cents: null, group_ids: [], variants: [] },
  { ...base, id: 4, kind: "simple", names: n("Coca"), price_cents: 1200, happy_hour_price_cents: null, group_ids: [], variants: [] },
  { ...base, id: 9, kind: "combo", names: n("Menu Classic"), price_cents: 6500, happy_hour_price_cents: 5900, group_ids: [4], variants: [], station_id: null,
    slots: [
      { names: n("Burger"), options: [
        { product_id: 1, variant_id: 101, extra_cents: 0, is_default: true },
        { product_id: 1, variant_id: 102, extra_cents: 1500, is_default: false },
      ] },
      { names: n("Side"), options: [
        { product_id: 2, variant_id: null, extra_cents: 0, is_default: true },
        { product_id: 3, variant_id: null, extra_cents: 300, is_default: false },
      ] },
      { names: n("Drink"), options: [{ product_id: 4, variant_id: null, extra_cents: 0, is_default: false }] },
    ] },
];
const menu: Menu = { stations: [], categories: [], products, modifier_groups: groups };
const ix = new MenuIndex(menu);

describe("names", () => {
  it("falls back to French", () => {
    expect(pickName(n("Classic", "كلاسيك"), "ar")).toBe("كلاسيك");
    expect(pickName(n("Classic"), "ar")).toBe("Classic");
    expect(pickName({ fr: "Frites", en: "  " }, "en")).toBe("Frites");
    expect(pickName(undefined, "fr")).toBe("");
  });
});

describe("happy hour", () => {
  const w = { enabled: true, start: 15 * 60, end: 18 * 60 };
  it("inside / outside a day window (end exclusive)", () => {
    expect(isHappyHour(15 * 60, w)).toBe(true);
    expect(isHappyHour(17 * 60 + 59, w)).toBe(true);
    expect(isHappyHour(18 * 60, w)).toBe(false);
    expect(isHappyHour(14 * 60 + 59, w)).toBe(false);
  });
  it("window crossing midnight", () => {
    const night = { enabled: true, start: 23 * 60, end: 60 };
    expect(isHappyHour(23 * 60 + 30, night)).toBe(true);
    expect(isHappyHour(30, night)).toBe(true);
    expect(isHappyHour(60, night)).toBe(false);
    expect(isHappyHour(12 * 60, night)).toBe(false);
  });
  it("disabled or empty window is never happy hour", () => {
    expect(isHappyHour(16 * 60, { ...w, enabled: false })).toBe(false);
    expect(isHappyHour(16 * 60, { enabled: true, start: 600, end: 600 })).toBe(false);
  });
  it("uses happy-hour price only where one is set", () => {
    expect(basePrice(products[0], 101, true)).toBe(3900);
    expect(basePrice(products[0], 102, true)).toBe(6000); // no HH price on Double
    expect(basePrice(products[0], 101, false)).toBe(4500);
    expect(basePrice(products[1], null, true)).toBe(1000);
  });
});

describe("defaults and validation", () => {
  it("default config picks first active variant and default options", () => {
    const c = defaultConfig(ix, 1);
    expect(c).toEqual({ product_id: 1, variant_id: 101, modifiers: [{ option_id: 12, qty: 1 }] });
    expect(validateConfig(ix, c)).toEqual([]);
  });
  it("required group missing", () => {
    const c = { product_id: 1, variant_id: 101, modifiers: [] };
    expect(validateConfig(ix, c)).toEqual([{ kind: "min", group_id: 1, need: 1 }]);
  });
  it("too many in a group and per-option limits", () => {
    const c = { product_id: 1, variant_id: 101, modifiers: [{ option_id: 12, qty: 1 }, { option_id: 21, qty: 2 }, { option_id: 22, qty: 1 }, { option_id: 31, qty: 1 }] };
    expect(validateConfig(ix, c)).toEqual([]);
    const tooMany = { ...c, modifiers: [{ option_id: 12, qty: 1 }, { option_id: 21, qty: 3 }, { option_id: 22, qty: 1 }] };
    expect(validateConfig(ix, tooMany)).toEqual(
      expect.arrayContaining([{ kind: "option_qty", option_id: 21, max: 2 }, { kind: "max", group_id: 2, max: 3 }]),
    );
  });
  it("inactive options, foreign options and inactive variants are rejected", () => {
    expect(validateConfig(ix, { product_id: 1, variant_id: 101, modifiers: [{ option_id: 12, qty: 1 }, { option_id: 23, qty: 1 }] }))
      .toContainEqual({ kind: "unknown_option", option_id: 23 });
    expect(validateConfig(ix, { product_id: 2, variant_id: null, modifiers: [{ option_id: 21, qty: 1 }] }))
      .toContainEqual({ kind: "unknown_option", option_id: 21 });
    expect(validateConfig(ix, { product_id: 1, variant_id: 103, modifiers: [{ option_id: 12, qty: 1 }] }))
      .toContainEqual({ kind: "variant_required", product_id: 1 });
  });
  it("86'd product cannot be sold", () => {
    const m2 = new MenuIndex({ ...menu, products: products.map((p) => (p.id === 2 ? { ...p, available: false } : p)) });
    expect(validateConfig(m2, defaultConfig(m2, 2))).toContainEqual({ kind: "unavailable", product_id: 2 });
  });
});

describe("toggleModifier", () => {
  it("single-choice group behaves like radio buttons", () => {
    let c = defaultConfig(ix, 1);
    c = toggleModifier(ix, c, 11, 1);
    expect(c.modifiers).toEqual([{ option_id: 11, qty: 1 }]);
  });
  it("extras stack up to max_qty and group max", () => {
    let c = defaultConfig(ix, 1);
    c = toggleModifier(ix, c, 21, 1);
    c = toggleModifier(ix, c, 21, 1);
    c = toggleModifier(ix, c, 21, 1); // capped at 2
    expect(c.modifiers.find((m) => m.option_id === 21)?.qty).toBe(2);
    c = toggleModifier(ix, c, 22, 1); // group now at 3 = max
    c = toggleModifier(ix, c, 22, 1);
    expect(c.modifiers.find((m) => m.option_id === 22)?.qty).toBe(1);
    c = toggleModifier(ix, c, 21, -1);
    c = toggleModifier(ix, c, 21, -1);
    expect(c.modifiers.find((m) => m.option_id === 21)).toBeUndefined();
    expect(validateConfig(ix, c)).toEqual([]);
  });
});

describe("unit price", () => {
  it("simple product: variant + extras, removals free", () => {
    let c = defaultConfig(ix, 1, 102);
    c = toggleModifier(ix, c, 21, 1);
    c = toggleModifier(ix, c, 21, 1);
    c = toggleModifier(ix, c, 31, 1);
    expect(unitPrice(ix, c)).toBe(6000 + 1000);
    expect(unitPrice(ix, defaultConfig(ix, 1), true)).toBe(3900);
  });
  it("combo: base + size + slot extras + paid extras on parts", () => {
    const c = defaultConfig(ix, 9);
    expect(c.combo?.map((x) => x.product_id)).toEqual([1, 2, 4]); // drink has no default -> first option
    expect(validateConfig(ix, c)).toEqual([]);
    expect(unitPrice(ix, c)).toBe(6500);
    const upgraded = {
      ...toggleModifier(ix, c, 42, 1),
      combo: [
        toggleModifier(ix, { ...c.combo![0], variant_id: 102 }, 21, 1),
        { product_id: 3, variant_id: null, modifiers: [] },
        c.combo![2],
      ],
    };
    // 65 + Maxi 10 + Double 15 + cheese 5 + potatoes 3
    expect(unitPrice(ix, upgraded)).toBe(6500 + 1000 + 1500 + 500 + 300);
    expect(unitPrice(ix, c, true)).toBe(5900);
  });
  it("combo with a choice that is not in the slot is invalid", () => {
    const c = defaultConfig(ix, 9);
    const bad = { ...c, combo: [c.combo![0], { product_id: 4, variant_id: null, modifiers: [] }, c.combo![2]] };
    expect(validateConfig(ix, bad)).toContainEqual({ kind: "slot_choice", slot: 1 });
    const missing = { ...c, combo: [c.combo![0]] };
    expect(validateConfig(ix, missing)).toEqual([{ kind: "slot_choice", slot: 1 }, { kind: "slot_choice", slot: 2 }]);
  });
});

describe("describe", () => {
  it("lists variant, choices, extras with qty and removals", () => {
    let c = defaultConfig(ix, 1, 102);
    c = toggleModifier(ix, c, 21, 1);
    c = toggleModifier(ix, c, 21, 1);
    c = toggleModifier(ix, c, 31, 1);
    expect(describeItem(ix, c, "fr")).toEqual(["Double", "À point", "+ Fromage ×2", "− Sans oignon"]);
    expect(describeItem(ix, c, "en")).toContain("+ Cheese ×2");
  });
  it("combo parts are listed with their own details", () => {
    const lines = describeItem(ix, defaultConfig(ix, 9), "fr");
    expect(lines).toEqual(["Normal", "• Classic (Simple, À point)", "• Frites", "• Coca"]);
  });
});
