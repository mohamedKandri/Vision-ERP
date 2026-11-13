import { Minus, Plus } from "lucide-react";
import { useMemo, useState } from "react";
import { useT } from "../i18n";
import {
  defaultConfig,
  describe,
  pickName,
  toggleModifier,
  unitPrice,
  validateConfig,
  type ItemConfig,
  type MenuIndex,
  type ModifierGroup,
  type Product,
} from "../lib/menu";
import { formatMoney } from "../lib/money";
import { Button, Modal } from "./ui";

/**
 * Popup to pick variant, modifiers and combo choices for one product.
 * Used by the POS (Phase 3) and as a preview in the menu back office.
 */
export function ItemConfigurator({
  ix,
  productId,
  happy = false,
  initial,
  onClose,
  onConfirm,
}: {
  ix: MenuIndex;
  productId: number;
  happy?: boolean;
  initial?: ItemConfig;
  onClose: () => void;
  onConfirm: (cfg: ItemConfig, unitCents: number) => void;
}) {
  const t = useT();
  const product = ix.product(productId);
  const [cfg, setCfg] = useState<ItemConfig>(() => initial ?? defaultConfig(ix, productId));
  const [editingPart, setEditingPart] = useState<number | null>(null);
  const errors = useMemo(() => validateConfig(ix, cfg), [ix, cfg]);
  const price = unitPrice(ix, cfg, happy);

  if (editingPart !== null && cfg.combo) {
    const part = cfg.combo[editingPart];
    return (
      <Modal onClose={() => setEditingPart(null)}>
        <ConfigBody
          ix={ix}
          product={ix.product(part.product_id)}
          cfg={part}
          onChange={(c) => setCfg({ ...cfg, combo: cfg.combo!.map((x, i) => (i === editingPart ? c : x)) })}
          hidePrice
        />
        <Button variant="primary" className="mt-4 w-full" onClick={() => setEditingPart(null)}>
          {t("back")}
        </Button>
      </Modal>
    );
  }

  return (
    <Modal onClose={onClose}>
      <div className="flex items-start justify-between gap-3">
        <h2 className="text-xl font-bold">{pickName(product.names, t.lang)}</h2>
        <span className="text-xl font-bold text-brand" dir="ltr">{formatMoney(price, t.lang)}</span>
      </div>
      <ConfigBody ix={ix} product={product} cfg={cfg} onChange={setCfg} />

      {product.kind === "combo" &&
        product.slots.map((slot, si) => {
          const part = cfg.combo?.[si];
          return (
            <section key={si} className="mt-4">
              <h3 className="mb-2 font-semibold">{pickName(slot.names, t.lang)}</h3>
              <div className="grid grid-cols-2 gap-2">
                {slot.options.map((o, oi) => {
                  const p = ix.products.get(o.product_id);
                  if (!p) return null;
                  const v = p.variants.find((x) => x.id === o.variant_id);
                  const selected = part?.product_id === o.product_id && (o.variant_id === null || part?.variant_id === o.variant_id);
                  const disabled = !p.available || !p.active;
                  return (
                    <button
                      key={oi}
                      disabled={disabled}
                      onClick={() => setCfg({ ...cfg, combo: (cfg.combo ?? []).map((x, i) => (i === si ? defaultConfig(ix, o.product_id, o.variant_id) : x)) })}
                      className={`min-h-14 rounded-xl border px-3 py-2 text-start disabled:opacity-40 ${selected ? "border-brand bg-brand/20" : "line panel-2"}`}
                    >
                      <div className="font-medium">
                        {pickName(p.names, t.lang)}
                        {v ? ` · ${pickName(v.names, t.lang)}` : ""}
                      </div>
                      <div className="muted text-xs">
                        {disabled ? t("out_of_stock") : o.extra_cents ? `+${formatMoney(o.extra_cents, t.lang)}` : ""}
                      </div>
                    </button>
                  );
                })}
              </div>
              {part && ix.groupsOf(ix.product(part.product_id)).length > 0 && (
                <button className="mt-2 min-h-12 w-full rounded-xl border line text-sm" onClick={() => setEditingPart(si)}>
                  {t("customize")}
                  {describe(ix, part, t.lang).length ? ` — ${describe(ix, part, t.lang).join(", ")}` : ""}
                </button>
              )}
            </section>
          );
        })}

      {errors.length > 0 && (
        <p className="mt-4 text-sm text-red-500">
          {errors
            .map((e) => {
              if (e.kind === "min") {
                const g = ix.groups.get(e.group_id);
                return t("err_select_min", { n: e.need, g: g ? pickName(g.names, t.lang) : "" });
              }
              if (e.kind === "unavailable") return `${pickName(ix.product(e.product_id).names, t.lang)}: ${t("out_of_stock")}`;
              return t("err_invalid");
            })
            .join(" · ")}
        </p>
      )}

      <div className="mt-5 flex gap-3">
        <Button className="flex-1" onClick={onClose}>
          {t("cancel")}
        </Button>
        <Button variant="primary" className="flex-[2]" disabled={errors.length > 0} onClick={() => onConfirm(cfg, price)}>
          {t("add_to_order")} · <span dir="ltr">{formatMoney(price, t.lang)}</span>
        </Button>
      </div>
    </Modal>
  );
}

function rule(t: ReturnType<typeof useT>, g: ModifierGroup) {
  if (g.min_select === 1 && g.max_select === 1) return `${t("required")} · ${t("choose_one")}`;
  if (g.min_select === 0) return t("choose_up_to", { n: g.max_select });
  return t("choose_between", { min: g.min_select, max: g.max_select });
}

function ConfigBody({
  ix,
  product,
  cfg,
  onChange,
  hidePrice,
}: {
  ix: MenuIndex;
  product: Product;
  cfg: ItemConfig;
  onChange: (c: ItemConfig) => void;
  hidePrice?: boolean;
}) {
  const t = useT();
  const variants = product.variants.filter((v) => v.active);
  return (
    <div>
      {hidePrice && <h2 className="text-xl font-bold">{pickName(product.names, t.lang)}</h2>}
      {variants.length > 0 && (
        <section className="mt-4">
          <div className="grid grid-cols-3 gap-2">
            {variants.map((v) => (
              <button
                key={v.id}
                onClick={() => onChange({ ...cfg, variant_id: v.id })}
                className={`min-h-14 rounded-xl border font-semibold ${cfg.variant_id === v.id ? "border-brand bg-brand/20" : "line panel-2"}`}
              >
                {pickName(v.names, t.lang)}
                {!hidePrice && <div className="muted text-xs font-normal" dir="ltr">{formatMoney(v.price_cents, t.lang)}</div>}
              </button>
            ))}
          </div>
        </section>
      )}
      {ix.groupsOf(product).map((g) => (
        <section key={g.id} className="mt-4">
          <div className="mb-2 flex items-baseline justify-between gap-2">
            <h3 className="font-semibold">{pickName(g.names, t.lang)}</h3>
            <span className="muted text-xs">{rule(t, g)}</span>
          </div>
          <div className="grid grid-cols-2 gap-2">
            {g.options
              .filter((o) => o.active)
              .map((o) => {
                const qty = cfg.modifiers.find((m) => m.option_id === o.id)?.qty ?? 0;
                const label = (
                  <span>
                    {pickName(o.names, t.lang)}
                    {o.price_cents > 0 && (
                      <span className="muted ms-1 text-xs" dir="ltr">
                        +{formatMoney(o.price_cents, t.lang)}
                      </span>
                    )}
                  </span>
                );
                if (o.max_qty > 1) {
                  return (
                    <div key={o.id} className={`flex min-h-14 items-center justify-between gap-2 rounded-xl border px-2 ${qty ? "border-brand bg-brand/20" : "line panel-2"}`}>
                      <span className="ps-1 text-sm">{label}</span>
                      <div className="flex items-center gap-1" dir="ltr">
                        <button aria-label="-" className="h-11 w-11 rounded-lg border line" onClick={() => onChange(toggleModifier(ix, cfg, o.id, -1))}>
                          <Minus className="mx-auto h-4 w-4" />
                        </button>
                        <span className="w-6 text-center font-bold">{qty}</span>
                        <button aria-label="+" className="h-11 w-11 rounded-lg border line" onClick={() => onChange(toggleModifier(ix, cfg, o.id, 1))}>
                          <Plus className="mx-auto h-4 w-4" />
                        </button>
                      </div>
                    </div>
                  );
                }
                return (
                  <button
                    key={o.id}
                    aria-pressed={qty > 0}
                    onClick={() => onChange(toggleModifier(ix, cfg, o.id, qty > 0 && g.min_select === 0 ? -1 : qty > 0 && g.max_select > 1 ? -1 : 1))}
                    className={`min-h-14 rounded-xl border px-3 text-start text-sm ${qty ? "border-brand bg-brand/20 font-semibold" : "line panel-2"}`}
                  >
                    {label}
                  </button>
                );
              })}
          </div>
        </section>
      ))}
    </div>
  );
}
