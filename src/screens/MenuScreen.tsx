import { ArrowDown, ArrowUp, Eye, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ItemConfigurator } from "../components/ItemConfigurator";
import { NamesField } from "../components/NamesField";
import { Button, Card, Field, Modal, MoneyField, Select, shrinkImage, Tabs, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT } from "../i18n";
import { MenuIndex, pickName, type Category, type ComboSlot, type Menu, type ModifierGroup, type ModifierOption, type Names, type Product } from "../lib/menu";
import { formatMoney, splitIncluded, percentOf } from "../lib/money";
import { rpc, type Settings, type TaxRow } from "../lib/rpc";

export const CATEGORY_ICONS: Record<string, string> = {
  burger: "🍔", fries: "🍟", drink: "🥤", dessert: "🍰", sauce: "🥫", combo: "🍱", chicken: "🍗", salad: "🥗", coffee: "☕", star: "⭐",
};

function useMenu(all: boolean) {
  const [menu, setMenu] = useState<Menu | null>(null);
  const load = useCallback(() => rpc<Menu>("menu.get", { all }).then(setMenu), [all]);
  useEffect(() => void load(), [load]);
  return { menu, load };
}

type Tab = "products" | "categories" | "modifiers";

export function MenuScreen() {
  const t = useT();
  const toast = useToast();
  const [tab, setTab] = useState<Tab>("products");
  const { menu, load } = useMenu(true);
  const [taxes, setTaxes] = useState<TaxRow[]>([]);
  const [defaultTax, setDefaultTax] = useState<number | null>(null);
  useEffect(() => {
    rpc<TaxRow[]>("taxes.list").then(setTaxes);
    rpc<Settings>("settings.get").then((s) => setDefaultTax(Number(s.default_tax_id) || null));
  }, []);

  if (!menu) return <p className="muted p-6">{t("loading")}</p>;
  const fail = (e: unknown) => toast.error(errorText(t.lang, e));

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 p-4">
      <Tabs<Tab>
        value={tab}
        onChange={setTab}
        tabs={[
          { id: "products", label: t("tab_products") },
          { id: "categories", label: t("tab_categories") },
          { id: "modifiers", label: t("tab_modifiers") },
        ]}
      />
      {tab === "products" && <ProductsTab menu={menu} taxes={taxes} defaultTax={defaultTax} reload={load} toast={toast} fail={fail} />}
      {tab === "categories" && <CategoriesTab menu={menu} reload={load} toast={toast} fail={fail} />}
      {tab === "modifiers" && <ModifiersTab menu={menu} reload={load} toast={toast} fail={fail} />}
      {toast.node}
    </div>
  );
}

type Ctx = { menu: Menu; reload: () => Promise<void>; toast: ReturnType<typeof useToast>; fail: (e: unknown) => void };

async function move(table: string, list: { id: number }[], index: number, dir: -1 | 1, reload: () => Promise<void>) {
  const j = index + dir;
  if (j < 0 || j >= list.length) return;
  const ids = list.map((x) => x.id);
  [ids[index], ids[j]] = [ids[j], ids[index]];
  await rpc("menu.reorder", { table, ids });
  await reload();
}

function ReorderButtons({ onUp, onDown }: { onUp: () => void; onDown: () => void }) {
  const t = useT();
  return (
    <div className="flex shrink-0 gap-1">
      <button aria-label={t("move_up")} className="h-11 w-11 rounded-lg border line" onClick={(e) => (e.stopPropagation(), onUp())}>
        <ArrowUp className="mx-auto h-4 w-4" />
      </button>
      <button aria-label={t("move_down")} className="h-11 w-11 rounded-lg border line" onClick={(e) => (e.stopPropagation(), onDown())}>
        <ArrowDown className="mx-auto h-4 w-4" />
      </button>
    </div>
  );
}

// ================================================================== products

function priceLabel(p: Product, lang: ReturnType<typeof useT>["lang"]) {
  const vs = p.variants.filter((v) => v.active);
  if (vs.length === 0) return formatMoney(p.price_cents, lang);
  const min = Math.min(...vs.map((v) => v.price_cents));
  const max = Math.max(...vs.map((v) => v.price_cents));
  return min === max ? formatMoney(min, lang) : `${formatMoney(min, lang)} – ${formatMoney(max, lang)}`;
}

function ProductsTab({ menu, taxes, defaultTax, reload, toast, fail }: Ctx & { taxes: TaxRow[]; defaultTax: number | null }) {
  const t = useT();
  const [cat, setCat] = useState<number | "all">("all");
  const [q, setQ] = useState("");
  const [edit, setEdit] = useState<Product | null>(null);
  const [preview, setPreview] = useState<number | null>(null);
  const ix = useMemo(() => new MenuIndex(menu), [menu]);

  const list = menu.products.filter(
    (p) =>
      (cat === "all" || p.category_id === cat) &&
      (!q.trim() || Object.values(p.names).some((n) => n?.toLowerCase().includes(q.trim().toLowerCase()))),
  );

  const blank = (kind: Product["kind"]): Product => ({
    id: 0, kind, category_id: cat === "all" ? menu.categories[0]?.id ?? 0 : cat, names: { fr: "" }, description: {},
    price_cents: 0, happy_hour_price_cents: null,
    // The shop's default tax (Settings → Sales), not simply the first one in the list.
    tax_id: taxes.find((x) => x.active && x.id === defaultTax)?.id ?? taxes.find((x) => x.active && x.rate_bp > 0)?.id ?? taxes[0]?.id ?? 1,
    station_id: kind === "combo" ? null : menu.stations[0]?.id ?? null, photo: "", active: true, available: true,
    sort: menu.products.length, variants: [], group_ids: [], slots: [],
  });

  const seed = async () => {
    try {
      await rpc("menu.seed_demo");
      await reload();
    } catch (e) {
      fail(e);
    }
  };

  return (
    <>
      <Card>
        <div className="flex flex-wrap items-center gap-2">
          <TextInput className="max-w-xs" placeholder={t("search")} value={q} onChange={(e) => setQ(e.target.value)} />
          <div className="ms-auto flex gap-2">
            <Button variant="primary" disabled={!menu.categories.length} onClick={() => setEdit(blank("simple"))}>
              <Plus className="inline h-4 w-4" /> {t("new_product")}
            </Button>
            <Button disabled={!menu.categories.length} onClick={() => setEdit(blank("combo"))}>
              <Plus className="inline h-4 w-4" /> {t("new_combo")}
            </Button>
          </div>
        </div>
        <div className="mt-3 flex gap-2 overflow-x-auto">
          <CatChip active={cat === "all"} onClick={() => setCat("all")} color="#78716c" label={t("all")} />
          {menu.categories.map((c) => (
            <CatChip key={c.id} active={cat === c.id} onClick={() => setCat(c.id)} color={c.color} label={`${CATEGORY_ICONS[c.icon] ?? ""} ${pickName(c.names, t.lang)}`} />
          ))}
        </div>
      </Card>

      {menu.products.length === 0 ? (
        <Card>
          <p className="muted mb-3">{t("menu_empty")}</p>
          <Button variant="primary" onClick={seed}>{t("load_demo_menu")}</Button>
        </Card>
      ) : (
        <div className="overflow-x-auto border line bg-[var(--panel)]">
          <div className="label grid min-w-[760px] grid-cols-[minmax(0,2.2fr)_minmax(0,1fr)_120px_120px_110px_110px] gap-px border-b line bg-[var(--panel-2)]">
            <span className="px-3 py-2">{t("name")}</span><span className="px-3 py-2">{t("category")}</span><span className="px-3 py-2 text-end">{t("price")}</span>
            <span className="px-3 py-2">{t("station")}</span><span className="px-3 py-2">{t("available")}</span><span className="px-3 py-2" />
          </div>
          {list.map((p, i) => {
            const c = menu.categories.find((x) => x.id === p.category_id);
            const st = menu.stations.find((x) => x.id === p.station_id);
            return (
              <div key={p.id} className={`grid min-w-[760px] grid-cols-[minmax(0,2.2fr)_minmax(0,1fr)_120px_120px_110px_110px] items-center border-b line text-[13px] hover:bg-[var(--panel-2)] ${p.active ? "" : "opacity-50"}`}>
                <button className="flex min-w-0 items-center gap-3 px-3 py-2 text-start" onClick={() => setEdit(p)}>
                  <span className="h-8 w-1.5 shrink-0" style={{ background: c?.color }} />
                  <span className="min-w-0 truncate font-semibold">{pickName(p.names, t.lang)}</span>
                  {p.kind === "combo" && <span className="label shrink-0 text-emerald-600">{t("kind_combo")}</span>}
                  {p.variants.some((v) => v.active) && <span className="muted shrink-0 text-xs">{p.variants.filter((v) => v.active).map((v) => pickName(v.names, t.lang)).join(" · ")}</span>}
                </button>
                <span className="muted truncate px-3">{pickName(c?.names, t.lang)}</span>
                <span className="px-3 text-end font-semibold" dir="ltr">{priceLabel(p, t.lang)}</span>
                <span className="muted px-3">{st ? pickName(st.names, t.lang) : "—"}</span>
                <span className={`px-3 font-semibold ${p.available ? "text-emerald-600" : "text-red-600"}`}>{p.available ? t("available") : t("out_of_stock")}</span>
                <span className="flex justify-end gap-1 px-2">
                  <button aria-label={t("preview")} className="h-9 w-9 rounded-sm border line" onClick={() => setPreview(p.id)}><Eye className="mx-auto h-4 w-4" /></button>
                  {cat !== "all" && <ReorderButtons onUp={() => move("products", list, i, -1, reload)} onDown={() => move("products", list, i, 1, reload)} />}
                </span>
              </div>
            );
          })}
        </div>
      )}

      {edit && (
        <ProductEditor
          initial={edit}
          menu={menu}
          taxes={taxes}
          onClose={() => setEdit(null)}
          onSaved={async () => {
            setEdit(null);
            toast.ok(t("saved"));
            await reload();
          }}
          fail={fail}
        />
      )}
      {preview !== null && (
        <ItemConfigurator ix={ix} productId={preview} onClose={() => setPreview(null)} onConfirm={() => setPreview(null)} />
      )}
    </>
  );
}

function CatChip({ active, onClick, color, label }: { active: boolean; onClick: () => void; color: string; label: string }) {
  return (
    <button
      onClick={onClick}
      className={`min-h-12 shrink-0 rounded-xl border-2 px-4 font-medium ${active ? "text-white" : ""}`}
      style={{ borderColor: color, background: active ? color : "transparent" }}
    >
      {label}
    </button>
  );
}

function ProductEditor({
  initial,
  menu,
  taxes,
  onClose,
  onSaved,
  fail,
}: {
  initial: Product;
  menu: Menu;
  taxes: TaxRow[];
  onClose: () => void;
  onSaved: () => void;
  fail: (e: unknown) => void;
}) {
  const t = useT();
  const [p, setP] = useState<Product>(structuredClone(initial));
  const isNew = p.id === 0;
  const tax = taxes.find((x) => x.id === p.tax_id);
  const simpleProducts = menu.products.filter((x) => x.kind === "simple" && x.id !== p.id);

  const save = async () => {
    try {
      await rpc("products.save", {
        ...p,
        id: isNew ? null : p.id,
        variants: p.variants.map((v) => ({ ...v, id: v.id > 0 ? v.id : null })),
        slots: p.slots.map((s) => ({ names: s.names, options: s.options })),
      });
      onSaved();
    } catch (e) {
      fail(e);
    }
  };

  const vatHint = (cents: number) => {
    if (!tax) return "";
    const s = tax.price_include ? splitIncluded(cents, tax.rate_bp) : { base: cents, tax: percentOf(cents, tax.rate_bp) };
    return t("ht_equiv", { ht: formatMoney(s.base, t.lang), tva: formatMoney(s.tax, t.lang) });
  };

  const setSlot = (i: number, s: ComboSlot) => setP({ ...p, slots: p.slots.map((x, j) => (j === i ? s : x)) });

  return (
    <Modal onClose={onClose} wide>
      <h2 className="mb-4 text-xl font-bold">
        {isNew ? (p.kind === "combo" ? t("new_combo") : t("new_product")) : pickName(p.names, t.lang)}
      </h2>
      <div className="flex flex-col gap-4">
        <NamesField value={p.names} onChange={(n) => setP({ ...p, names: n as Names })} autoFocus={isNew} />
        <div className="grid gap-3 sm:grid-cols-3">
          <Field label={t("category")}>
            <Select<number> value={p.category_id} onChange={(v) => setP({ ...p, category_id: v })} options={menu.categories.map((c) => ({ value: c.id, label: pickName(c.names, t.lang) }))} />
          </Field>
          <Field label={t("tax")}>
            <Select<number> value={p.tax_id} onChange={(v) => setP({ ...p, tax_id: v })} options={taxes.filter((x) => x.active || x.id === p.tax_id).map((x) => ({ value: x.id, label: x.name }))} />
          </Field>
          {p.kind === "simple" && (
            <Field label={t("station")}>
              <Select<number> value={p.station_id ?? 0} onChange={(v) => setP({ ...p, station_id: v || null })}
                options={[{ value: 0, label: t("no_station") }, ...menu.stations.map((s) => ({ value: s.id, label: pickName(s.names, t.lang) }))]} />
            </Field>
          )}
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("price")} hint={vatHint(p.price_cents)}>
            <MoneyField cents={p.price_cents} onChange={(c) => setP({ ...p, price_cents: c ?? 0 })} />
          </Field>
          <Field label={t("happy_hour_price")}>
            <MoneyField nullable cents={p.happy_hour_price_cents} onChange={(c) => setP({ ...p, happy_hour_price_cents: c })} />
          </Field>
        </div>

        <Field label={t("photo")}>
          <div className="flex items-center gap-3">
            {p.photo && <img src={p.photo} alt="" className="h-16 w-16 rounded-lg object-cover" />}
            <label className="panel-2 flex min-h-12 cursor-pointer items-center rounded-xl border line px-4">
              {t("choose_image")}
              <input type="file" accept="image/png,image/jpeg,image/webp" className="hidden"
                onChange={(e) => { const f = e.target.files?.[0]; if (f) shrinkImage(f, 320).then((photo) => setP({ ...p, photo })); }} />
            </label>
            {p.photo && <Button variant="plain" onClick={() => setP({ ...p, photo: "" })}>{t("remove")}</Button>}
          </div>
        </Field>

        <div className="grid gap-2 sm:grid-cols-2">
          <Toggle checked={p.active} onChange={(v) => setP({ ...p, active: v })} label={t("active")} />
          <Toggle checked={p.available} onChange={(v) => setP({ ...p, available: v })} label={t("available")} />
        </div>

        {p.kind === "simple" && (
          <section>
            <h3 className="font-semibold">{t("variants")}</h3>
            <p className="muted mb-2 text-sm">{t("variants_hint")}</p>
            <div className="flex flex-col gap-2">
              {p.variants.map((v, i) =>
                v.active ? (
                  <div key={v.id || `n${i}`} className="panel-2 grid items-end gap-2 rounded-xl border line p-2 sm:grid-cols-[1fr_1fr_1fr_8rem_8rem_auto]">
                    <TextInput dir="ltr" placeholder="FR" value={v.names.fr} onChange={(e) => setP({ ...p, variants: p.variants.map((x, j) => (j === i ? { ...x, names: { ...x.names, fr: e.target.value } } : x)) })} />
                    <TextInput dir="rtl" placeholder="AR" value={v.names.ar ?? ""} onChange={(e) => setP({ ...p, variants: p.variants.map((x, j) => (j === i ? { ...x, names: { ...x.names, ar: e.target.value } } : x)) })} />
                    <TextInput dir="ltr" placeholder="EN" value={v.names.en ?? ""} onChange={(e) => setP({ ...p, variants: p.variants.map((x, j) => (j === i ? { ...x, names: { ...x.names, en: e.target.value } } : x)) })} />
                    <MoneyField cents={v.price_cents} onChange={(c) => setP({ ...p, variants: p.variants.map((x, j) => (j === i ? { ...x, price_cents: c ?? 0 } : x)) })} />
                    <MoneyField nullable placeholder="HH" cents={v.happy_hour_price_cents} onChange={(c) => setP({ ...p, variants: p.variants.map((x, j) => (j === i ? { ...x, happy_hour_price_cents: c } : x)) })} />
                    <button aria-label={t("remove")} className="h-12 w-12 rounded-lg border line"
                      onClick={() => setP({ ...p, variants: v.id > 0 ? p.variants.map((x, j) => (j === i ? { ...x, active: false } : x)) : p.variants.filter((_, j) => j !== i) })}>
                      <Trash2 className="mx-auto h-4 w-4" />
                    </button>
                  </div>
                ) : null,
              )}
              <Button onClick={() => setP({ ...p, variants: [...p.variants, { id: -Date.now(), names: { fr: "" }, price_cents: p.price_cents, happy_hour_price_cents: null, active: true }] })}>
                <Plus className="inline h-4 w-4" /> {t("add_variant")}
              </Button>
            </div>
          </section>
        )}

        <section>
          <h3 className="mb-2 font-semibold">{t("modifier_groups")}</h3>
          <div className="flex flex-wrap gap-2">
            {menu.modifier_groups.filter((g) => g.active).map((g) => {
              const on = p.group_ids.includes(g.id);
              return (
                <button key={g.id} aria-pressed={on}
                  onClick={() => setP({ ...p, group_ids: on ? p.group_ids.filter((x) => x !== g.id) : [...p.group_ids, g.id] })}
                  className={`min-h-12 rounded-xl border px-4 ${on ? "border-brand bg-brand/20 font-semibold" : "line panel-2"}`}>
                  {pickName(g.names, t.lang)}
                </button>
              );
            })}
          </div>
        </section>

        {p.kind === "combo" && (
          <section>
            <h3 className="mb-2 font-semibold">{t("combo_slots")}</h3>
            <div className="flex flex-col gap-3">
              {p.slots.map((s, si) => (
                <div key={si} className="panel-2 rounded-xl border line p-3">
                  <div className="flex items-end gap-2">
                    <div className="flex-1"><NamesField value={s.names} onChange={(n) => setSlot(si, { ...s, names: n as Names })} /></div>
                    <button aria-label={t("remove")} className="h-12 w-12 rounded-lg border line" onClick={() => setP({ ...p, slots: p.slots.filter((_, j) => j !== si) })}>
                      <Trash2 className="mx-auto h-4 w-4" />
                    </button>
                  </div>
                  <div className="mt-3 flex flex-col gap-2">
                    {s.options.map((o, oi) => {
                      const prod = menu.products.find((x) => x.id === o.product_id);
                      const setO = (patch: Partial<typeof o>) => setSlot(si, { ...s, options: s.options.map((x, j) => (j === oi ? { ...x, ...patch } : x)) });
                      return (
                        <div key={oi} className="grid items-center gap-2 sm:grid-cols-[2fr_1.2fr_8rem_auto_auto]">
                          <Select<number> value={o.product_id} onChange={(v) => setO({ product_id: v, variant_id: null })}
                            options={simpleProducts.map((x) => ({ value: x.id, label: pickName(x.names, t.lang) }))} />
                          {prod && prod.variants.filter((v) => v.active).length > 0 ? (
                            <Select<number> value={o.variant_id ?? 0} onChange={(v) => setO({ variant_id: v || null })}
                              options={[{ value: 0, label: "—" }, ...prod.variants.filter((v) => v.active).map((v) => ({ value: v.id, label: pickName(v.names, t.lang) }))]} />
                          ) : <span />}
                          <MoneyField cents={o.extra_cents} onChange={(c) => setO({ extra_cents: c ?? 0 })} />
                          <label className="flex min-h-12 items-center gap-2 px-2 text-sm">
                            <input type="radio" className="h-5 w-5" checked={o.is_default}
                              onChange={() => setSlot(si, { ...s, options: s.options.map((x, j) => ({ ...x, is_default: j === oi })) })} />
                            {t("default")}
                          </label>
                          <button aria-label={t("remove")} className="h-12 w-12 rounded-lg border line" onClick={() => setSlot(si, { ...s, options: s.options.filter((_, j) => j !== oi) })}>
                            <Trash2 className="mx-auto h-4 w-4" />
                          </button>
                        </div>
                      );
                    })}
                    <Button disabled={!simpleProducts.length}
                      onClick={() => setSlot(si, { ...s, options: [...s.options, { product_id: simpleProducts[0].id, variant_id: null, extra_cents: 0, is_default: s.options.length === 0 }] })}>
                      <Plus className="inline h-4 w-4" /> {t("add_choice")}
                    </Button>
                  </div>
                </div>
              ))}
              <Button onClick={() => setP({ ...p, slots: [...p.slots, { names: { fr: "" }, options: [] }] })}>
                <Plus className="inline h-4 w-4" /> {t("add_slot")}
              </Button>
            </div>
          </section>
        )}

        <div className="sticky bottom-0 -mx-5 -mb-5 flex justify-end gap-3 border-t line panel p-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" onClick={save} disabled={!p.names.fr?.trim()}>{t("save")}</Button>
        </div>
      </div>
    </Modal>
  );
}

// ================================================================== categories

function CategoriesTab({ menu, reload, toast, fail }: Ctx) {
  const t = useT();
  const [edit, setEdit] = useState<Category | null>(null);
  const save = async () => {
    if (!edit) return;
    try {
      await rpc("categories.save", { ...edit, id: edit.id || null });
      setEdit(null);
      toast.ok(t("saved"));
      await reload();
    } catch (e) {
      fail(e);
    }
  };
  return (
    <Card actions={<Button variant="primary" onClick={() => setEdit({ id: 0, names: { fr: "" }, color: "#f59e0b", icon: "burger", sort: menu.categories.length, active: true })}><Plus className="inline h-4 w-4" /> {t("new_category")}</Button>}>
      <div className="flex flex-col divide-y line">
        {menu.categories.map((c, i) => (
          <div key={c.id} className={`flex min-h-16 items-center gap-3 py-2 ${c.active ? "" : "opacity-50"}`}>
            <button className="flex flex-1 items-center gap-3 text-start" onClick={() => setEdit(c)}>
              <span className="flex h-12 w-12 items-center justify-center rounded-lg text-2xl" style={{ background: c.color }}>{CATEGORY_ICONS[c.icon]}</span>
              <span className="font-medium">{pickName(c.names, t.lang)}</span>
              <span className="muted text-sm">{menu.products.filter((p) => p.category_id === c.id).length}</span>
            </button>
            <ReorderButtons onUp={() => move("categories", menu.categories, i, -1, reload)} onDown={() => move("categories", menu.categories, i, 1, reload)} />
          </div>
        ))}
      </div>
      {edit && (
        <Modal onClose={() => setEdit(null)}>
          <div className="flex flex-col gap-4">
            <NamesField value={edit.names} onChange={(n) => setEdit({ ...edit, names: n as Names })} autoFocus />
            <div className="grid grid-cols-2 gap-3">
              <Field label={t("color")}>
                <input type="color" className="h-12 w-full rounded-xl border line" value={edit.color} onChange={(e) => setEdit({ ...edit, color: e.target.value })} />
              </Field>
              <Field label={t("icon")}>
                <Select<string> value={edit.icon} onChange={(v) => setEdit({ ...edit, icon: v })} options={Object.entries(CATEGORY_ICONS).map(([k, v]) => ({ value: k, label: `${v} ${k}` }))} />
              </Field>
            </div>
            <Toggle checked={edit.active} onChange={(v) => setEdit({ ...edit, active: v })} label={t("active")} />
            <div className="flex justify-end gap-3">
              <Button onClick={() => setEdit(null)}>{t("cancel")}</Button>
              <Button variant="primary" onClick={save} disabled={!edit.names.fr?.trim()}>{t("save")}</Button>
            </div>
          </div>
        </Modal>
      )}
    </Card>
  );
}

// ================================================================== modifier groups

function ModifiersTab({ menu, reload, toast, fail }: Ctx) {
  const t = useT();
  const [edit, setEdit] = useState<ModifierGroup | null>(null);
  const save = async () => {
    if (!edit) return;
    try {
      await rpc("modifier_groups.save", { ...edit, id: edit.id || null, options: edit.options.map((o) => ({ ...o, id: o.id > 0 ? o.id : null })) });
      setEdit(null);
      toast.ok(t("saved"));
      await reload();
    } catch (e) {
      fail(e);
    }
  };
  const newOpt = (): ModifierOption => ({ id: -Date.now(), names: { fr: "" }, kind: "choice", price_cents: 0, max_qty: 1, is_default: false, active: true });
  const setOpt = (i: number, patch: Partial<ModifierOption>) => edit && setEdit({ ...edit, options: edit.options.map((o, j) => (j === i ? { ...o, ...patch } : o)) });

  return (
    <Card actions={<Button variant="primary" onClick={() => setEdit({ id: 0, names: { fr: "" }, min_select: 0, max_select: 1, sort: menu.modifier_groups.length, active: true, options: [newOpt()] })}><Plus className="inline h-4 w-4" /> {t("new_group")}</Button>}>
      <div className="flex flex-col divide-y line">
        {menu.modifier_groups.map((g, i) => (
          <div key={g.id} className={`flex min-h-16 items-center gap-3 py-2 ${g.active ? "" : "opacity-50"}`}>
            <button className="flex-1 text-start" onClick={() => setEdit(structuredClone(g))}>
              <div className="font-medium">{pickName(g.names, t.lang)}</div>
              <div className="muted text-sm">
                {g.min_select}–{g.max_select} · {g.options.filter((o) => o.active).map((o) => pickName(o.names, t.lang)).join(", ")}
              </div>
            </button>
            <ReorderButtons onUp={() => move("modifier_groups", menu.modifier_groups, i, -1, reload)} onDown={() => move("modifier_groups", menu.modifier_groups, i, 1, reload)} />
          </div>
        ))}
      </div>
      {edit && (
        <Modal onClose={() => setEdit(null)} wide>
          <div className="flex flex-col gap-4">
            <NamesField value={edit.names} onChange={(n) => setEdit({ ...edit, names: n as Names })} autoFocus={!edit.id} />
            <div className="grid grid-cols-2 gap-3 sm:grid-cols-3">
              <Field label={t("min_select")}>
                <TextInput type="number" min={0} value={edit.min_select} onChange={(e) => setEdit({ ...edit, min_select: Math.max(0, Math.trunc(Number(e.target.value) || 0)) })} />
              </Field>
              <Field label={t("max_select")}>
                <TextInput type="number" min={1} value={edit.max_select} onChange={(e) => setEdit({ ...edit, max_select: Math.max(1, Math.trunc(Number(e.target.value) || 1)) })} />
              </Field>
              <div className="col-span-2 sm:col-span-1 sm:self-end">
                <Toggle checked={edit.active} onChange={(v) => setEdit({ ...edit, active: v })} label={t("active")} />
              </div>
            </div>
            <h3 className="font-semibold">{t("options")}</h3>
            {edit.options.map((o, i) =>
              o.active ? (
                <div key={o.id} className="panel-2 grid items-center gap-2 rounded-xl border line p-2 sm:grid-cols-[1fr_1fr_1fr_8rem_7rem_5rem_auto_auto]">
                  <TextInput dir="ltr" placeholder="FR" value={o.names.fr} onChange={(e) => setOpt(i, { names: { ...o.names, fr: e.target.value } })} />
                  <TextInput dir="rtl" placeholder="AR" value={o.names.ar ?? ""} onChange={(e) => setOpt(i, { names: { ...o.names, ar: e.target.value } })} />
                  <TextInput dir="ltr" placeholder="EN" value={o.names.en ?? ""} onChange={(e) => setOpt(i, { names: { ...o.names, en: e.target.value } })} />
                  <Select<ModifierOption["kind"]> value={o.kind} onChange={(k) => setOpt(i, { kind: k, price_cents: k === "remove" ? 0 : o.price_cents })}
                    options={[{ value: "choice", label: t("kind_choice") }, { value: "extra", label: t("kind_extra") }, { value: "remove", label: t("kind_remove") }]} />
                  {o.kind === "remove" ? <span className="muted text-center">0</span> : <MoneyField cents={o.price_cents} onChange={(c) => setOpt(i, { price_cents: c ?? 0 })} />}
                  <TextInput type="number" min={1} max={20} title={t("max_qty")} value={o.max_qty} onChange={(e) => setOpt(i, { max_qty: Math.min(20, Math.max(1, Math.trunc(Number(e.target.value) || 1))) })} />
                  <label className="flex min-h-12 items-center gap-2 px-1 text-sm">
                    <input type="checkbox" className="h-5 w-5" checked={o.is_default} onChange={(e) => setOpt(i, { is_default: e.target.checked })} />
                    {t("default")}
                  </label>
                  <button aria-label={t("remove")} className="h-12 w-12 rounded-lg border line"
                    onClick={() => setEdit({ ...edit, options: o.id > 0 ? edit.options.map((x, j) => (j === i ? { ...x, active: false } : x)) : edit.options.filter((_, j) => j !== i) })}>
                    <Trash2 className="mx-auto h-4 w-4" />
                  </button>
                </div>
              ) : null,
            )}
            <Button onClick={() => setEdit({ ...edit, options: [...edit.options, newOpt()] })}>
              <Plus className="inline h-4 w-4" /> {t("add_option")}
            </Button>
            <div className="flex justify-end gap-3">
              <Button onClick={() => setEdit(null)}>{t("cancel")}</Button>
              <Button variant="primary" onClick={save} disabled={!edit.names.fr?.trim()}>{t("save")}</Button>
            </div>
          </div>
        </Modal>
      )}
    </Card>
  );
}

// ================================================================== 86 board

/** Quick out-of-stock board for cashiers and the kitchen during service. */
export function AvailabilityScreen() {
  const t = useT();
  const toast = useToast();
  const { menu, load } = useMenu(false);
  if (!menu) return <p className="muted p-6">{t("loading")}</p>;
  const flip = async (p: Product) => {
    try {
      await rpc("products.set_available", { id: p.id, available: !p.available });
      await load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 p-4">
      {menu.categories.map((c) => {
        const ps = menu.products.filter((p) => p.category_id === c.id);
        if (!ps.length) return null;
        return (
          <Card key={c.id} title={`${CATEGORY_ICONS[c.icon] ?? ""} ${pickName(c.names, t.lang)}`}>
            <div className="grid grid-cols-2 gap-2 sm:grid-cols-3 lg:grid-cols-4">
              {ps.map((p) => (
                <button key={p.id} onClick={() => flip(p)} aria-pressed={!p.available}
                  className={`min-h-16 rounded-xl border-2 px-3 py-2 text-start ${p.available ? "line panel-2" : "border-red-600 bg-red-600/15"}`}>
                  <div className={`font-medium ${p.available ? "" : "line-through"}`}>{pickName(p.names, t.lang)}</div>
                  <div className={`text-xs ${p.available ? "text-emerald-500" : "font-bold text-red-500"}`}>{p.available ? t("available") : t("out_of_stock")}</div>
                </button>
              ))}
            </div>
          </Card>
        );
      })}
      {toast.node}
    </div>
  );
}
