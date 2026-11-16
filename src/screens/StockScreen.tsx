import { AlertTriangle, ClipboardList, Minus, Pencil, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { NamesField } from "../components/NamesField";
import { Button, Card, Field, Modal, MoneyField, Select, Tabs, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { useLive } from "../lib/live";
import { pickName, type Menu, type Names, type Product } from "../lib/menu";
import { formatMoney, formatRate } from "../lib/money";
import { rpc, type SessionUser } from "../lib/rpc";
import {
  costUnit, foodCostBp, formatQty, marginBp, marginLevel, parseQty, qtyInput, recipeCost, unitLabel, UNITS,
  type Ingredient, type RecipeLine, type Unit,
} from "../lib/stock";

type Tab = "stock" | "recipes" | "counts" | "moves";

export function StockScreen({ me }: { me: SessionUser }) {
  const t = useT();
  const manage = me.permissions.includes("inventory.manage");
  const [tab, setTab] = useState<Tab>("stock");
  const tabs: { id: Tab; label: string }[] = manage
    ? [
        { id: "stock", label: t("st_tab_stock") },
        { id: "recipes", label: t("st_tab_recipes") },
        { id: "counts", label: t("st_tab_counts") },
        { id: "moves", label: t("st_tab_moves") },
      ]
    : [{ id: "stock", label: t("st_tab_stock") }];
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4 p-4">
      {tabs.length > 1 && <Tabs value={tab} onChange={setTab} tabs={tabs} />}
      {tab === "stock" && <StockTab manage={manage} />}
      {tab === "recipes" && <RecipesTab />}
      {tab === "counts" && <CountsTab />}
      {tab === "moves" && <MovesTab />}
    </div>
  );
}

function useIngredients() {
  const [rows, setRows] = useState<Ingredient[] | null>(null);
  const load = useCallback(() => rpc<Ingredient[]>("ingredients.list").then(setRows, () => {}), []);
  useEffect(() => void load(), [load]);
  useLive(["stock", "orders"], load);
  return { rows, load };
}

/** Text field holding a quantity in the ingredient's unit; reports milli-units (or null when empty/invalid). */
function QtyField({ milli, onChange, unit, allowNegative, autoFocus, placeholder }: {
  milli: number | null; onChange: (m: number | null) => void; unit: Unit; allowNegative?: boolean; autoFocus?: boolean; placeholder?: string;
}) {
  const t = useT();
  const [text, setText] = useState(milli === null ? "" : qtyInput(milli));
  useEffect(() => {
    if (milli === null ? text.trim() !== "" && parseQty(text, allowNegative) !== null : parseQty(text, allowNegative) !== milli) {
      setText(milli === null ? "" : qtyInput(milli));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [milli]);
  const bad = text.trim() !== "" && parseQty(text, allowNegative) === null;
  return (
    <div className="flex items-center gap-2">
      <TextInput
        inputMode="decimal"
        dir="ltr"
        value={text}
        autoFocus={autoFocus}
        placeholder={placeholder}
        aria-invalid={bad}
        className={bad ? "!ring-2 !ring-red-500" : ""}
        onChange={(e) => {
          setText(e.target.value);
          onChange(e.target.value.trim() === "" ? null : parseQty(e.target.value, allowNegative));
        }}
      />
      <span className="muted w-12 shrink-0 text-sm">{unitLabel(unit, t.lang)}</span>
    </div>
  );
}

// ---------------------------------------------------------------- stock list

function StockTab({ manage }: { manage: boolean }) {
  const t = useT();
  const toast = useToast();
  const { rows, load } = useIngredients();
  const [q, setQ] = useState("");
  const [onlyLow, setOnlyLow] = useState(false);
  const [edit, setEdit] = useState<Partial<Ingredient> | null>(null);
  const [adjust, setAdjust] = useState<Ingredient | null>(null);
  const [waste, setWaste] = useState<{ ingredient?: Ingredient } | null>(null);

  if (!rows) return <p className="muted">{t("loading")}</p>;
  const norm = (x: string) => x.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
  const shown = rows.filter((r) => (!onlyLow || r.low || r.out) && (!q.trim() || norm(pickName(r.names, t.lang) + " " + r.names.fr).includes(norm(q.trim()))));
  const alerts = rows.filter((r) => r.active && (r.low || r.out));
  const total = rows.reduce((s, r) => s + r.value_cents, 0);

  const seed = async () => {
    try {
      await rpc("stock.seed_demo");
      toast.ok(t("saved"));
      load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  return (
    <>
      {alerts.length > 0 && (
        <button onClick={() => setOnlyLow(true)} className="flex min-h-12 items-center gap-3 rounded-xl bg-amber-500/15 px-4 text-start text-amber-700 dark:text-amber-300">
          <AlertTriangle className="shrink-0" />
          <span className="font-medium">{t("st_alerts", { n: alerts.length })}</span>
          <span className="truncate text-sm opacity-80">{alerts.slice(0, 6).map((a) => pickName(a.names, t.lang)).join(" · ")}</span>
        </button>
      )}
      <Card
        title={t("nav_stock")}
        actions={
          <div className="flex flex-wrap justify-end gap-2">
            <Button onClick={() => setWaste({})}><Trash2 className="inline h-4 w-4" /> {t("st_waste")}</Button>
            {manage && (
              <Button variant="primary" onClick={() => setEdit({ unit: "g", cost_cents: 0, min_milli: 0, active: true })}>
                <Plus className="inline h-4 w-4" /> {t("add")}
              </Button>
            )}
          </div>
        }
      >
        <div className="mb-3 flex flex-wrap items-center gap-3">
          <div className="min-w-48 flex-1">
            <TextInput value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("search")} aria-label={t("search")} />
          </div>
          <div className="w-64"><Toggle checked={onlyLow} onChange={setOnlyLow} label={t("st_only_low")} /></div>
          {manage && <span className="muted text-sm">{t("st_total_value")}: <b className="num">{formatMoney(total, t.lang)}</b></span>}
        </div>
        {rows.length === 0 ? (
          <div className="flex flex-col items-center gap-3 py-8 text-center">
            <p className="muted">{t("st_empty")}</p>
            {manage && <Button onClick={seed}>{t("st_seed_demo")}</Button>}
          </div>
        ) : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="muted">
                <tr className="text-start">
                  <th className="py-2 text-start">{t("name")}</th>
                  <th className="text-end">{t("st_in_stock")}</th>
                  <th className="text-end">{t("st_min")}</th>
                  {manage && <th className="text-end">{t("st_cost")}</th>}
                  {manage && <th className="text-end">{t("st_value")}</th>}
                  <th />
                </tr>
              </thead>
              <tbody>
                {shown.map((r) => (
                  <tr key={r.id} className={`border-t line ${r.active ? "" : "opacity-50"}`}>
                    <td className="py-2 pe-3">
                      <span className="font-medium">{pickName(r.names, t.lang)}</span>
                      {r.out ? (
                        <span className="ms-2 rounded bg-red-600 px-1.5 py-0.5 text-xs text-white">{t("st_out")}</span>
                      ) : r.low ? (
                        <span className="ms-2 rounded bg-amber-500 px-1.5 py-0.5 text-xs text-white">{t("st_low")}</span>
                      ) : null}
                    </td>
                    <td className={`num text-end whitespace-nowrap ${r.stock_milli < 0 ? "text-red-500" : ""}`}>{formatQty(r.stock_milli, r.unit, t.lang)}</td>
                    <td className="num muted text-end whitespace-nowrap">{r.min_milli ? formatQty(r.min_milli, r.unit, t.lang) : "—"}</td>
                    {manage && <td className="num muted text-end whitespace-nowrap">{formatMoney(r.cost_cents, t.lang)}/{costUnit(r.unit, t.lang)}</td>}
                    {manage && <td className="num text-end whitespace-nowrap">{formatMoney(r.value_cents, t.lang)}</td>}
                    <td className="py-1 text-end whitespace-nowrap">
                      <button className="m-0.5 inline-flex h-11 w-11 items-center justify-center rounded-lg border line panel-2" aria-label={t("st_waste")} title={t("st_waste")} onClick={() => setWaste({ ingredient: r })}>
                        <Trash2 className="h-4 w-4" />
                      </button>
                      {manage && (
                        <>
                          <button className="m-0.5 inline-flex h-11 w-11 items-center justify-center rounded-lg border line panel-2" aria-label={t("st_adjust")} title={t("st_adjust")} onClick={() => setAdjust(r)}>
                            <Plus className="h-3 w-3" /><Minus className="h-3 w-3" />
                          </button>
                          <button className="m-0.5 inline-flex h-11 w-11 items-center justify-center rounded-lg border line panel-2" aria-label={t("edit")} title={t("edit")} onClick={() => setEdit(r)}>
                            <Pencil className="h-4 w-4" />
                          </button>
                        </>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            {!shown.length && <p className="muted py-4 text-center">—</p>}
          </div>
        )}
      </Card>
      {edit && <IngredientDialog value={edit} onClose={() => setEdit(null)} onSaved={() => { setEdit(null); toast.ok(t("saved")); load(); }} />}
      {adjust && <AdjustDialog ing={adjust} onClose={() => setAdjust(null)} onDone={() => { setAdjust(null); toast.ok(t("saved")); load(); }} />}
      {waste && (
        <WasteDialog
          ingredients={rows.filter((r) => r.active)}
          initial={waste.ingredient}
          onClose={() => setWaste(null)}
          onDone={(v) => { setWaste(null); toast.ok(t("st_waste_done", { v: formatMoney(v, t.lang) })); load(); }}
        />
      )}
      {toast.node}
    </>
  );
}

function IngredientDialog({ value, onClose, onSaved }: { value: Partial<Ingredient>; onClose: () => void; onSaved: () => void }) {
  const t = useT();
  const [v, setV] = useState(value);
  const [initial, setInitial] = useState<number | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const unit = (v.unit ?? "g") as Unit;
  const isNew = v.id === undefined;
  const save = async () => {
    try {
      await rpc("ingredients.save", {
        id: v.id ?? null, names: v.names, unit, cost_cents: v.cost_cents ?? 0, min_milli: v.min_milli ?? 0,
        active: v.active ?? true, initial_milli: isNew ? initial ?? 0 : 0,
      });
      onSaved();
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  return (
    <Modal onClose={onClose}>
      <h2 className="mb-3 text-lg font-semibold">{isNew ? t("st_new_ingredient") : pickName(v.names, t.lang)}</h2>
      <div className="flex flex-col gap-3">
        <NamesField value={v.names ?? { fr: "" }} onChange={(n) => setV({ ...v, names: n as Names })} autoFocus />
        <div className="grid grid-cols-2 gap-3">
          <Field label={t("st_unit")} hint={!isNew && (v.recipe_count ?? 0) > 0 ? t("st_unit_locked") : undefined}>
            <Select<Unit> value={unit} onChange={(u) => setV({ ...v, unit: u })} options={UNITS.map((u) => ({ value: u, label: t(`st_unit_${u}` as Key) }))} />
          </Field>
          <Field label={t("st_cost_per", { u: costUnit(unit, t.lang) })}>
            <MoneyField cents={v.cost_cents ?? 0} onChange={(c) => setV({ ...v, cost_cents: c ?? 0 })} />
          </Field>
          <Field label={t("st_min")} hint={t("st_min_hint")}>
            <QtyField unit={unit} milli={v.min_milli ?? 0} onChange={(m) => setV({ ...v, min_milli: m ?? 0 })} />
          </Field>
          {isNew && (
            <Field label={t("st_initial")}>
              <QtyField unit={unit} milli={initial} onChange={setInitial} placeholder="0" />
            </Field>
          )}
        </div>
        <Toggle checked={v.active ?? true} onChange={(a) => setV({ ...v, active: a })} label={t("active")} />
        {err && <p className="text-sm text-red-500">{err}</p>}
        <div className="mt-2 flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" onClick={save} disabled={!v.names?.fr?.trim()}>{t("save")}</Button>
        </div>
      </div>
    </Modal>
  );
}

function ReasonChips({ value, onChange, keys }: { value: string; onChange: (s: string) => void; keys: Key[] }) {
  const t = useT();
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap gap-2">
        {keys.map((k) => (
          <button key={k} type="button" onClick={() => onChange(t(k))} className={`min-h-11 rounded-lg border line px-3 text-sm ${value === t(k) ? "bg-brand text-white" : "panel-2"}`}>
            {t(k)}
          </button>
        ))}
      </div>
      <TextInput value={value} onChange={(e) => onChange(e.target.value)} placeholder={t("st_reason")} maxLength={200} aria-label={t("st_reason")} />
    </div>
  );
}

function AdjustDialog({ ing, onClose, onDone }: { ing: Ingredient; onClose: () => void; onDone: () => void }) {
  const t = useT();
  const [dir, setDir] = useState<"in" | "out">("in");
  const [qty, setQty] = useState<number | null>(null);
  const [reason, setReason] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const after = ing.stock_milli + (qty ?? 0) * (dir === "in" ? 1 : -1);
  const save = async () => {
    try {
      await rpc("stock.adjust", { ingredient_id: ing.id, delta_milli: (qty ?? 0) * (dir === "in" ? 1 : -1), reason });
      onDone();
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  return (
    <Modal onClose={onClose}>
      <h2 className="mb-1 text-lg font-semibold">{t("st_adjust")}: {pickName(ing.names, t.lang)}</h2>
      <p className="muted mb-3 text-sm">{t("st_in_stock")}: <span className="num">{formatQty(ing.stock_milli, ing.unit, t.lang)}</span></p>
      <div className="flex flex-col gap-3">
        <Tabs value={dir} onChange={setDir} tabs={[{ id: "in", label: t("st_adjust_in") }, { id: "out", label: t("st_adjust_out") }]} />
        <QtyField unit={ing.unit} milli={qty} onChange={setQty} autoFocus />
        <p className="muted text-sm">{t("st_after")}: <b className="num">{formatQty(after, ing.unit, t.lang)}</b></p>
        <ReasonChips value={reason} onChange={setReason} keys={["st_r_delivery", "st_r_correction", "st_r_transfer", "st_r_staff_meal"]} />
        {err && <p className="text-sm text-red-500">{err}</p>}
        <div className="mt-2 flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" onClick={save} disabled={!qty || qty <= 0 || !reason.trim()}>{t("save")}</Button>
        </div>
      </div>
    </Modal>
  );
}

function WasteDialog({ ingredients, initial, onClose, onDone }: { ingredients: Ingredient[]; initial?: Ingredient; onClose: () => void; onDone: (valueCents: number) => void }) {
  const t = useT();
  const [mode, setMode] = useState<"ingredient" | "product">(initial ? "ingredient" : "product");
  const [ingId, setIngId] = useState<number>(initial?.id ?? ingredients[0]?.id ?? 0);
  const [qtyMilli, setQtyMilli] = useState<number | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [pid, setPid] = useState<number>(0);
  const [vid, setVid] = useState<number | null>(null);
  const [n, setN] = useState(1);
  const [reason, setReason] = useState("");
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    rpc<Menu>("menu.get").then((m) => {
      setMenu(m);
      const first = m.products.find((p) => p.kind === "simple");
      if (first) {
        setPid(first.id);
        setVid(first.variants[0]?.id ?? null);
      }
    }, () => {});
  }, []);
  const ing = ingredients.find((i) => i.id === ingId);
  const products = (menu?.products ?? []).filter((p) => p.kind === "simple");
  const product = products.find((p) => p.id === pid);
  const ok = reason.trim() && (mode === "ingredient" ? !!ing && !!qtyMilli && qtyMilli > 0 : !!product && n >= 1);
  const save = async () => {
    try {
      const params = mode === "ingredient"
        ? { ingredient_id: ingId, qty_milli: qtyMilli, reason }
        : { product_id: pid, variant_id: vid, qty: n, reason };
      const r = await rpc<{ value_cents: number }>("stock.waste", params);
      onDone(r.value_cents);
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  return (
    <Modal onClose={onClose}>
      <h2 className="mb-3 text-lg font-semibold">{t("st_waste")}</h2>
      <div className="flex flex-col gap-3">
        <Tabs value={mode} onChange={setMode} tabs={[{ id: "product", label: t("st_waste_product") }, { id: "ingredient", label: t("st_waste_ingredient") }]} />
        {mode === "ingredient" ? (
          <>
            <Field label={t("st_ingredient")}>
              <Select<number> value={ingId} onChange={setIngId} options={ingredients.map((i) => ({ value: i.id, label: pickName(i.names, t.lang) }))} />
            </Field>
            {ing && <QtyField unit={ing.unit} milli={qtyMilli} onChange={setQtyMilli} autoFocus />}
          </>
        ) : (
          <>
            <Field label={t("st_product")}>
              <Select<number>
                value={pid}
                onChange={(p) => { setPid(p); setVid(products.find((x) => x.id === p)?.variants[0]?.id ?? null); }}
                options={products.map((p) => ({ value: p.id, label: pickName(p.names, t.lang) }))}
              />
            </Field>
            {product && product.variants.length > 0 && (
              <Field label={t("st_size")}>
                <Select<number> value={vid ?? product.variants[0].id} onChange={setVid} options={product.variants.map((v) => ({ value: v.id, label: pickName(v.names, t.lang) }))} />
              </Field>
            )}
            <Field label={t("st_qty")}>
              <div className="flex items-center gap-2">
                <Button onClick={() => setN(Math.max(1, n - 1))} aria-label="-"><Minus className="h-4 w-4" /></Button>
                <span className="num w-12 text-center text-xl font-semibold">{n}</span>
                <Button onClick={() => setN(Math.min(999, n + 1))} aria-label="+"><Plus className="h-4 w-4" /></Button>
              </div>
            </Field>
          </>
        )}
        <ReasonChips value={reason} onChange={setReason} keys={["st_r_expired", "st_r_dropped", "st_r_burnt", "st_r_wrong", "st_r_returned"]} />
        {err && <p className="text-sm text-red-500">{err}</p>}
        <div className="mt-2 flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="danger" onClick={save} disabled={!ok}>{t("st_waste_save")}</Button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- recipes & food cost

interface FoodCostRow {
  product_id: number;
  kind: "simple" | "combo";
  category_id: number;
  names: Names;
  variant_id: number | null;
  variant_names: Names | null;
  price_cents: number;
  price_ht_cents: number;
  cost_cents: number;
  margin_bp: number;
  has_recipe: boolean;
}
type RecipeMap = Record<"product" | "variant" | "option", Record<string, RecipeLine[]>>;
type Target = { target: "product" | "variant" | "option"; id: number; title: string; price?: { ht: number }; negative: boolean; combo?: boolean };

function MarginChip({ bp, has }: { bp: number; has: boolean }) {
  const t = useT();
  if (!has) return <span className="rounded bg-stone-500/20 px-2 py-0.5 text-xs">{t("st_no_recipe")}</span>;
  const lvl = marginLevel(bp);
  const cls = lvl === "good" ? "bg-emerald-600" : lvl === "watch" ? "bg-amber-500" : "bg-red-600";
  return <span className={`num rounded px-2 py-0.5 text-xs text-white ${cls}`}>{formatRate(bp)}</span>;
}

function RecipesTab() {
  const t = useT();
  const toast = useToast();
  const { rows: ingredients } = useIngredients();
  const [menu, setMenu] = useState<Menu | null>(null);
  const [fc, setFc] = useState<FoodCostRow[]>([]);
  const [recipes, setRecipes] = useState<RecipeMap | null>(null);
  const [edit, setEdit] = useState<Target | null>(null);
  const load = useCallback(() => {
    rpc<Menu>("menu.get").then(setMenu, () => {});
    rpc<FoodCostRow[]>("stock.food_cost").then(setFc, () => {});
    rpc<RecipeMap>("recipes.list").then(setRecipes, () => {});
  }, []);
  useEffect(load, [load]);
  useLive(["stock", "menu"], load);

  if (!menu || !recipes || !ingredients) return <p className="muted">{t("loading")}</p>;
  const byProduct = new Map<number, FoodCostRow[]>();
  for (const r of fc) byProduct.set(r.product_id, [...(byProduct.get(r.product_id) ?? []), r]);
  const noRecipe = fc.filter((r) => !r.has_recipe).length;

  const openProduct = (p: Product, r: FoodCostRow) => {
    // Sizes with their own recipe are edited separately; otherwise the product recipe.
    const sized = r.variant_id !== null && (recipes.variant[String(r.variant_id)]?.length || !recipes.product[String(p.id)]?.length);
    setEdit({
      target: sized ? "variant" : "product",
      id: sized ? r.variant_id! : p.id,
      title: pickName(p.names, t.lang) + (sized && r.variant_names ? ` · ${pickName(r.variant_names, t.lang)}` : ""),
      price: { ht: r.price_ht_cents },
      negative: false,
    });
  };

  return (
    <>
      <Card title={t("st_food_cost")}>
        <p className="muted mb-3 text-sm">{t("st_food_cost_hint")}{noRecipe > 0 && ` ${t("st_missing_recipes", { n: noRecipe })}`}</p>
        {menu.categories.map((c) => {
          const prods = menu.products.filter((p) => p.category_id === c.id);
          if (!prods.length) return null;
          return (
            <div key={c.id} className="mb-4">
              <h3 className="mb-1 font-semibold" style={{ color: c.color }}>{pickName(c.names, t.lang)}</h3>
              <div className="flex flex-col divide-y line">
                {prods.flatMap((p) =>
                  (byProduct.get(p.id) ?? []).map((r) => (
                    <button
                      key={`${p.id}-${r.variant_id}`}
                      onClick={() => (p.kind === "combo" ? setEdit({ target: "product", id: p.id, title: pickName(p.names, t.lang), negative: false, combo: true }) : openProduct(p, r))}
                      className="grid min-h-12 grid-cols-[1fr_auto_auto_auto] items-center gap-3 px-1 text-start"
                    >
                      <span>
                        <span className="font-medium">{pickName(p.names, t.lang)}</span>
                        {r.variant_names && <span className="muted"> · {pickName(r.variant_names, t.lang)}</span>}
                        {p.kind === "combo" && <span className="muted text-xs"> ({t("st_combo_default")})</span>}
                      </span>
                      <span className="num muted text-sm">{formatMoney(r.price_ht_cents, t.lang)} {t("excl")}</span>
                      <span className="num text-sm">{formatMoney(r.cost_cents, t.lang)}{r.has_recipe && <span className="muted"> · {formatRate(foodCostBp(r.price_ht_cents, r.cost_cents))}</span>}</span>
                      <MarginChip bp={r.margin_bp} has={r.has_recipe} />
                    </button>
                  )),
                )}
              </div>
            </div>
          );
        })}
      </Card>
      <Card title={t("st_option_recipes")}>
        <p className="muted mb-3 text-sm">{t("st_option_hint")}</p>
        {menu.modifier_groups.map((g) => (
          <div key={g.id} className="mb-3">
            <h3 className="mb-1 font-semibold">{pickName(g.names, t.lang)}</h3>
            <div className="flex flex-wrap gap-2">
              {g.options.map((o) => {
                const lines = recipes.option[String(o.id)] ?? [];
                const ingMap = new Map(ingredients.map((i) => [i.id, i]));
                return (
                  <button
                    key={o.id}
                    onClick={() => setEdit({ target: "option", id: o.id, title: `${pickName(g.names, t.lang)} · ${pickName(o.names, t.lang)}`, negative: true })}
                    className="panel-2 flex min-h-12 flex-col items-start justify-center rounded-lg border line px-3 text-start text-sm"
                  >
                    <span className="font-medium">{pickName(o.names, t.lang)}</span>
                    <span className="muted text-xs">
                      {lines.length
                        ? lines.map((l) => { const i = ingMap.get(l.ingredient_id); return i ? `${l.qty_milli > 0 ? "+" : ""}${formatQty(l.qty_milli, i.unit, t.lang)} ${pickName(i.names, t.lang)}` : ""; }).join(", ")
                        : "—"}
                    </span>
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </Card>
      {edit && (
        <RecipeEditor
          target={edit}
          ingredients={ingredients}
          lines={recipes[edit.target][String(edit.id)] ?? []}
          onClose={() => setEdit(null)}
          onSaved={() => { setEdit(null); toast.ok(t("saved")); load(); }}
        />
      )}
      {toast.node}
    </>
  );
}

function RecipeEditor({ target, ingredients, lines: initial, onClose, onSaved }: {
  target: Target; ingredients: Ingredient[]; lines: RecipeLine[]; onClose: () => void; onSaved: () => void;
}) {
  const t = useT();
  const active = ingredients.filter((i) => i.active || initial.some((l) => l.ingredient_id === i.id));
  const ingMap = useMemo(() => new Map(ingredients.map((i) => [i.id, i])), [ingredients]);
  const [lines, setLines] = useState<(RecipeLine & { key: number; qty: number | null })[]>(
    initial.map((l, k) => ({ ...l, key: k, qty: l.qty_milli })),
  );
  const [err, setErr] = useState<string | null>(null);
  const valid = lines.every((l) => l.qty !== null && (target.negative ? l.qty !== 0 : l.qty > 0)) && new Set(lines.map((l) => l.ingredient_id)).size === lines.length;
  const cost = recipeCost(lines.map((l) => ({ ingredient_id: l.ingredient_id, qty_milli: l.qty ?? 0 })), ingMap);
  const add = () => {
    const unused = active.find((i) => !lines.some((l) => l.ingredient_id === i.id));
    if (unused) setLines([...lines, { ingredient_id: unused.id, qty_milli: 0, qty: null, key: Date.now() }]);
  };
  const save = async () => {
    try {
      await rpc("recipes.save", { target: target.target, id: target.id, lines: lines.map((l) => ({ ingredient_id: l.ingredient_id, qty_milli: l.qty })) });
      onSaved();
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  return (
    <Modal onClose={onClose} wide>
      <h2 className="mb-1 text-lg font-semibold">{t("st_recipe")}: {target.title}</h2>
      <p className="muted mb-3 text-sm">{target.negative ? t("st_option_hint") : t("st_recipe_hint")}</p>
      <div className="flex flex-col gap-2">
        {lines.map((l, idx) => {
          const ing = ingMap.get(l.ingredient_id);
          return (
            <div key={l.key} className="grid grid-cols-[1fr_12rem_auto_auto] items-center gap-2">
              <Select<number>
                value={l.ingredient_id}
                onChange={(id) => setLines(lines.map((x, j) => (j === idx ? { ...x, ingredient_id: id } : x)))}
                options={active.map((i) => ({ value: i.id, label: pickName(i.names, t.lang) }))}
              />
              {ing && <QtyField unit={ing.unit} milli={l.qty} allowNegative={target.negative} onChange={(m) => setLines(lines.map((x, j) => (j === idx ? { ...x, qty: m } : x)))} />}
              <span className="num muted w-24 text-end text-sm">{ing && l.qty ? formatMoney(recipeCost([{ ingredient_id: ing.id, qty_milli: l.qty }], ingMap), t.lang) : ""}</span>
              <button className="inline-flex h-11 w-11 items-center justify-center rounded-lg border line panel-2" aria-label={t("st_delete")} onClick={() => setLines(lines.filter((_, j) => j !== idx))}>
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          );
        })}
        {!lines.length && <p className="muted py-2">{t("st_no_recipe")}</p>}
        <Button onClick={add} disabled={lines.length >= active.length} className="self-start"><Plus className="inline h-4 w-4" /> {t("st_add_line")}</Button>
        <div className="mt-2 flex flex-wrap items-center gap-4 border-t line pt-3">
          <span>{t("st_cost")}: <b className="num">{formatMoney(cost, t.lang)}</b></span>
          {target.price && (
            <>
              <span className="muted">{t("st_price_ht")}: <span className="num">{formatMoney(target.price.ht, t.lang)}</span></span>
              <span>{t("st_food_cost_pct")}: <b className="num">{formatRate(foodCostBp(target.price.ht, cost))}</b></span>
              <span className="flex items-center gap-2">{t("st_margin")}: <MarginChip bp={marginBp(target.price.ht, cost)} has /></span>
            </>
          )}
        </div>
        {target.combo && <p className="muted text-xs">{t("st_combo_note")}</p>}
        {err && <p className="text-sm text-red-500">{err}</p>}
        <div className="mt-2 flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" onClick={save} disabled={!valid}>{t("save")}</Button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- stock counts

interface CountRow { id: number; status: "open" | "done" | "cancelled"; note: string; created_at: string; closed_at: string | null; created_by: string; counted: number; lines: number; value_cents: number }
interface CountLine { ingredient_id: number; names: Names; unit: Unit; cost_cents: number; stock_milli: number; counted_milli: number | null; expected_milli: number | null }
interface CountDoc { id: number; status: CountRow["status"]; note: string; created_at: string; closed_at: string | null; lines: CountLine[] }

function CountsTab() {
  const t = useT();
  const toast = useToast();
  const [list, setList] = useState<CountRow[] | null>(null);
  const [open, setOpen] = useState<number | null>(null);
  const [note, setNote] = useState("");
  const load = useCallback(() => rpc<CountRow[]>("counts.list").then(setList, () => {}), []);
  useEffect(() => void load(), [load]);
  useLive(["stock"], load);
  const fmt = new Intl.DateTimeFormat(t.lang === "ar" ? "ar-MA-u-nu-latn" : t.lang, { dateStyle: "short", timeStyle: "short" });
  if (!list) return <p className="muted">{t("loading")}</p>;
  const current = list.find((c) => c.status === "open");
  const start = async () => {
    try {
      const r = await rpc<{ id: number }>("counts.start", { note });
      setNote("");
      setOpen(r.id);
      load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  if (open !== null) return <CountSheet id={open} onClose={() => { setOpen(null); load(); }} />;
  return (
    <>
      <Card title={t("st_tab_counts")}>
        <p className="muted mb-3 text-sm">{t("st_count_hint")}</p>
        {current ? (
          <Button variant="primary" onClick={() => setOpen(current.id)}><ClipboardList className="inline h-4 w-4" /> {t("st_count_continue")}</Button>
        ) : (
          <div className="flex flex-wrap gap-3">
            <div className="min-w-48 flex-1"><TextInput value={note} onChange={(e) => setNote(e.target.value)} placeholder={t("st_note")} maxLength={200} aria-label={t("st_note")} /></div>
            <Button variant="primary" onClick={start}><ClipboardList className="inline h-4 w-4" /> {t("st_count_start")}</Button>
          </div>
        )}
      </Card>
      <Card title={t("st_count_history")}>
        <div className="flex flex-col divide-y line">
          {list.map((c) => (
            <button key={c.id} onClick={() => setOpen(c.id)} className="grid min-h-12 grid-cols-[auto_1fr_auto_auto] items-center gap-3 px-1 text-start text-sm">
              <span className="num">{fmt.format(new Date(c.created_at))}</span>
              <span className="truncate">{c.created_by}{c.note && <span className="muted"> · {c.note}</span>}</span>
              <span className="muted">{t(`st_count_${c.status}` as Key)} · {c.counted}/{c.lines}</span>
              <span className={`num ${c.value_cents < 0 ? "text-red-500" : ""}`}>{c.status === "done" ? formatMoney(c.value_cents, t.lang) : ""}</span>
            </button>
          ))}
          {!list.length && <p className="muted py-3">—</p>}
        </div>
      </Card>
      {toast.node}
    </>
  );
}

function CountSheet({ id, onClose }: { id: number; onClose: () => void }) {
  const t = useT();
  const toast = useToast();
  const [doc, setDoc] = useState<CountDoc | null>(null);
  const [confirm, setConfirm] = useState(false);
  const [q, setQ] = useState("");
  const load = useCallback(() => rpc<CountDoc>("counts.get", { id }).then(setDoc, () => {}), [id]);
  useEffect(() => void load(), [load]);
  if (!doc) return <p className="muted">{t("loading")}</p>;
  const editable = doc.status === "open";
  const set = async (ingredient_id: number, counted_milli: number | null) => {
    setDoc({ ...doc, lines: doc.lines.map((l) => (l.ingredient_id === ingredient_id ? { ...l, counted_milli } : l)) });
    try {
      await rpc("counts.set", { id, ingredient_id, counted_milli });
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  const ref = (l: CountLine) => (editable ? l.stock_milli : l.expected_milli ?? l.stock_milli);
  const counted = doc.lines.filter((l) => l.counted_milli !== null);
  const diffValue = counted.reduce((s, l) => s + recipeCost([{ ingredient_id: 0, qty_milli: l.counted_milli! - ref(l) }], new Map([[0, l]])), 0);
  const changed = counted.filter((l) => l.counted_milli !== ref(l)).length;
  const norm = (x: string) => x.toLowerCase().normalize("NFD").replace(/[̀-ͯ]/g, "");
  const shown = doc.lines.filter((l) => !q.trim() || norm(pickName(l.names, t.lang) + " " + l.names.fr).includes(norm(q.trim())));
  const finish = async (action: "validate" | "cancel") => {
    try {
      await rpc(`counts.${action}`, { id });
      onClose();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  return (
    <Card
      title={`${t("st_count")} · ${doc.note || new Date(doc.created_at).toLocaleDateString()}`}
      actions={<Button onClick={onClose}>{t("back")}</Button>}
    >
      <div className="mb-3 flex flex-wrap items-center gap-3">
        <div className="min-w-48 flex-1"><TextInput value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("search")} aria-label={t("search")} /></div>
        <span className="text-sm">{t("st_count_progress", { n: counted.length, total: doc.lines.length })}</span>
        <span className={`num text-sm ${diffValue < 0 ? "text-red-500" : ""}`}>{t("st_count_diff")}: {formatMoney(diffValue, t.lang)}</span>
      </div>
      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead className="muted">
            <tr>
              <th className="py-2 text-start">{t("st_ingredient")}</th>
              <th className="text-end">{t("st_expected")}</th>
              <th className="w-64 text-start">{t("st_counted")}</th>
              <th className="text-end">{t("st_diff")}</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((l) => {
              const d = l.counted_milli === null ? null : l.counted_milli - ref(l);
              return (
                <tr key={l.ingredient_id} className="border-t line">
                  <td className="py-1 pe-3 font-medium">{pickName(l.names, t.lang)}</td>
                  <td className="num muted text-end whitespace-nowrap">{formatQty(ref(l), l.unit, t.lang)}</td>
                  <td className="ps-3">
                    {editable ? (
                      <QtyField unit={l.unit} milli={l.counted_milli} onChange={(m) => m !== null || l.counted_milli !== null ? set(l.ingredient_id, m) : undefined} placeholder="—" />
                    ) : (
                      <span className="num">{l.counted_milli === null ? "—" : formatQty(l.counted_milli, l.unit, t.lang)}</span>
                    )}
                  </td>
                  <td className={`num text-end whitespace-nowrap ${d && d < 0 ? "text-red-500" : d && d > 0 ? "text-emerald-600" : "muted"}`}>
                    {d === null ? "" : `${d > 0 ? "+" : ""}${formatQty(d, l.unit, t.lang)}`}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      {editable && (
        <div className="mt-4 flex flex-wrap justify-end gap-3">
          <Button variant="danger" onClick={() => finish("cancel")}>{t("st_count_cancel")}</Button>
          <Button variant="primary" onClick={() => setConfirm(true)} disabled={!counted.length}>{t("st_count_validate")}</Button>
        </div>
      )}
      {confirm && (
        <Modal onClose={() => setConfirm(false)}>
          <h2 className="mb-2 text-lg font-semibold">{t("st_count_validate")}</h2>
          <p className="mb-1">{t("st_count_confirm", { n: changed, counted: counted.length })}</p>
          <p className={`num mb-4 font-semibold ${diffValue < 0 ? "text-red-500" : ""}`}>{t("st_count_diff")}: {formatMoney(diffValue, t.lang)}</p>
          <div className="flex justify-end gap-3">
            <Button onClick={() => setConfirm(false)}>{t("cancel")}</Button>
            <Button variant="primary" onClick={() => finish("validate")}>{t("st_confirm")}</Button>
          </div>
        </Modal>
      )}
      {toast.node}
    </Card>
  );
}

// ---------------------------------------------------------------- movements

interface MoveRow {
  id: number; at: string; ingredient_id: number; ingredient_names: Names; unit: Unit; delta_milli: number; kind: string; reason: string;
  value_cents: number; order_number: number | null; product_names: Names | null; user_name: string | null;
}
const KINDS = ["sale", "return", "waste", "adjust", "count", "purchase", "initial"] as const;

function MovesTab() {
  const t = useT();
  const { rows: ingredients } = useIngredients();
  const [ing, setIng] = useState<number>(0);
  const [kind, setKind] = useState<string>("");
  const [rows, setRows] = useState<MoveRow[]>([]);
  const [done, setDone] = useState(false);
  const more = useCallback(
    (offset: number) =>
      rpc<MoveRow[]>("stock.history", { ingredient_id: ing || null, kind: kind || null, limit: 100, offset }).then((r) => {
        setRows((old) => (offset === 0 ? r : [...old, ...r]));
        setDone(r.length < 100);
      }, () => {}),
    [ing, kind],
  );
  useEffect(() => void more(0), [more]);
  const fmt = new Intl.DateTimeFormat(t.lang === "ar" ? "ar-MA-u-nu-latn" : t.lang, { dateStyle: "short", timeStyle: "short" });
  return (
    <Card title={t("st_tab_moves")}>
      <div className="mb-3 grid gap-3 sm:grid-cols-2">
        <Select<number> value={ing} onChange={setIng} options={[{ value: 0, label: t("st_all_ingredients") }, ...(ingredients ?? []).map((i) => ({ value: i.id, label: pickName(i.names, t.lang) }))]} />
        <Select<string> value={kind} onChange={setKind} options={[{ value: "", label: t("st_all_kinds") }, ...KINDS.map((k) => ({ value: k, label: t(`st_kind_${k}` as Key) }))]} />
      </div>
      <div className="overflow-x-auto">
        <table className="w-full text-sm">
          <thead className="muted">
            <tr>
              <th className="py-2 text-start">{t("audit_when")}</th>
              <th className="text-start">{t("st_ingredient")}</th>
              <th className="text-end">{t("st_qty")}</th>
              <th className="text-start ps-3">{t("st_kind")}</th>
              <th className="text-end">{t("st_value")}</th>
              <th className="text-start ps-3">{t("audit_who")}</th>
            </tr>
          </thead>
          <tbody>
            {rows.map((m) => (
              <tr key={m.id} className="border-t line align-top">
                <td className="num py-1.5 pe-3 whitespace-nowrap">{fmt.format(new Date(m.at))}</td>
                <td className="pe-3">{pickName(m.ingredient_names, t.lang)}</td>
                <td className={`num text-end whitespace-nowrap ${m.delta_milli < 0 ? "text-red-500" : "text-emerald-600"}`}>{m.delta_milli > 0 ? "+" : ""}{formatQty(m.delta_milli, m.unit, t.lang)}</td>
                <td className="ps-3">
                  {t(`st_kind_${m.kind}` as Key)}
                  {m.order_number !== null && <span className="muted"> · #{m.order_number}</span>}
                  {m.product_names && <span className="muted"> · {pickName(m.product_names, t.lang)}</span>}
                  {m.reason && <span className="muted"> · {m.reason}</span>}
                </td>
                <td className="num text-end whitespace-nowrap">{formatMoney(m.value_cents, t.lang)}</td>
                <td className="ps-3">{m.user_name ?? "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {!rows.length && <p className="muted py-3">—</p>}
      </div>
      {!done && <Button className="mt-3 w-full" onClick={() => more(rows.length)}>{t("load_more")}</Button>}
    </Card>
  );
}
