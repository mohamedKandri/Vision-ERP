import { useEffect, useState } from "react";
import { Button, Card, Field, Select, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, LANG_NAMES, useT, type Key } from "../i18n";
import { formatMoney, parseMoney } from "../lib/money";
import { rpc, type Lang, type Settings, type TaxRow } from "../lib/rpc";

type FieldDef =
  | { key: string; type: "text" | "textarea" }
  | { key: string; type: "int"; min?: number; max?: number }
  | { key: string; type: "bool" }
  | { key: string; type: "money" }
  | { key: string; type: "percent" }
  | { key: string; type: "enum"; options: { value: string; label: Key | string }[] }
  | { key: string; type: "tax" }
  | { key: string; type: "logo" }
  | { key: string; type: "time" };

const SECTIONS: { title: Key; fields: FieldDef[] }[] = [
  {
    title: "section_shop",
    fields: [
      { key: "shop_name", type: "text" },
      { key: "shop_address", type: "textarea" },
      { key: "shop_phone", type: "text" },
      { key: "shop_ice", type: "text" },
      { key: "shop_if", type: "text" },
      { key: "shop_rc", type: "text" },
      { key: "shop_patente", type: "text" },
      { key: "shop_logo", type: "logo" },
      { key: "default_language", type: "enum", options: (Object.keys(LANG_NAMES) as Lang[]).map((l) => ({ value: l, label: LANG_NAMES[l] })) },
    ],
  },
  {
    title: "section_receipt",
    fields: [
      { key: "receipt_header", type: "textarea" },
      { key: "receipt_footer", type: "textarea" },
      { key: "receipt_show_qr", type: "bool" },
    ],
  },
  {
    title: "section_sales",
    fields: [
      { key: "default_tax_id", type: "tax" },
      { key: "tax_rounding", type: "enum", options: [{ value: "per_line", label: "rounding_per_line" }, { value: "global", label: "rounding_global" }] },
      { key: "discount_pin_threshold_bp", type: "percent" },
      { key: "auto_lock_minutes", type: "int", min: 0, max: 240 },
      { key: "order_type_dine_in", type: "bool" },
      { key: "order_type_takeaway", type: "bool" },
      { key: "order_type_delivery", type: "bool" },
      { key: "delivery_fee_cents", type: "money" },
      { key: "print_kitchen_tickets", type: "bool" },
      { key: "auto_print_receipt", type: "bool" },
      { key: "drawer_on_cash", type: "bool" },
      { key: "stock_restore_on_void", type: "bool" },
      { key: "auto_86_on_stockout", type: "bool" },
    ],
  },
  {
    title: "section_happy_hour",
    fields: [
      { key: "happy_hour_enabled", type: "bool" },
      { key: "happy_hour_start_min", type: "time" },
      { key: "happy_hour_end_min", type: "time" },
    ],
  },
  {
    title: "section_kitchen",
    fields: [
      { key: "kds_warn_minutes", type: "int", min: 1, max: 120 },
      { key: "kds_late_minutes", type: "int", min: 1, max: 240 },
    ],
  },
  {
    title: "section_loyalty",
    fields: [
      { key: "loyalty_mode", type: "enum", options: [{ value: "none", label: "loyalty_none" }, { value: "stamps", label: "loyalty_stamps" }, { value: "points", label: "loyalty_points" }] },
      { key: "loyalty_stamps_needed", type: "int", min: 2, max: 100 },
      { key: "loyalty_points_per_mad", type: "int", min: 0, max: 100 },
      { key: "loyalty_point_value_cents", type: "money" },
    ],
  },
  {
    title: "section_backup",
    fields: [
      { key: "backup_daily", type: "bool" },
      { key: "backup_keep_days", type: "int", min: 1, max: 365 },
    ],
  },
];

function MoneyInput({ cents, onChange, lang }: { cents: number; onChange: (c: number) => void; lang: Lang }) {
  const [text, setText] = useState((cents / 100).toFixed(2));
  useEffect(() => setText((cents / 100).toFixed(2)), [cents]);
  const parsed = parseMoney(text);
  return (
    <TextInput
      inputMode="decimal"
      value={text}
      onChange={(e) => setText(e.target.value)}
      onBlur={() => (parsed !== null && parsed >= 0 ? onChange(parsed) : setText((cents / 100).toFixed(2)))}
      title={formatMoney(cents, lang)}
    />
  );
}

export function SettingsScreen({ canEdit, onSaved }: { canEdit: boolean; onSaved: (s: Settings) => void }) {
  const t = useT();
  const toast = useToast();
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings>({});
  const [taxes, setTaxes] = useState<TaxRow[]>([]);

  useEffect(() => {
    rpc<Settings>("settings.get").then((s) => {
      setSaved(s);
      setDraft(s);
    }, (e) => toast.error(errorText(t.lang, e)));
    rpc<TaxRow[]>("taxes.list").then(setTaxes);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  if (!saved) return <p className="muted p-6">{t("loading")}</p>;

  const set = (k: string, v: string | number | boolean) => setDraft((d) => ({ ...d, [k]: v }));
  const changed = Object.fromEntries(Object.entries(draft).filter(([k, v]) => saved[k] !== v));
  const dirty = Object.keys(changed).length > 0;

  const save = async () => {
    try {
      const s = await rpc<Settings>("settings.set", changed);
      setSaved(s);
      setDraft(s);
      onSaved(s);
      toast.ok(t("saved"));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  const label = (k: string) => t(k as Key);

  const render = (f: FieldDef) => {
    const v = draft[f.key];
    switch (f.type) {
      case "bool":
        return <Toggle key={f.key} checked={Boolean(v)} onChange={(b) => set(f.key, b)} label={label(f.key)} />;
      case "text":
        return (
          <Field key={f.key} label={label(f.key)}>
            <TextInput value={String(v ?? "")} onChange={(e) => set(f.key, e.target.value)} />
          </Field>
        );
      case "textarea":
        return (
          <Field key={f.key} label={label(f.key)}>
            <textarea className="min-h-20 w-full rounded-xl border line panel-2 p-3 outline-none focus:ring-2 focus:ring-brand" value={String(v ?? "")} onChange={(e) => set(f.key, e.target.value)} />
          </Field>
        );
      case "int":
        return (
          <Field key={f.key} label={label(f.key)}>
            <TextInput type="number" inputMode="numeric" min={f.min} max={f.max} value={Number(v ?? 0)} onChange={(e) => set(f.key, Math.trunc(Number(e.target.value) || 0))} />
          </Field>
        );
      case "percent":
        return (
          <Field key={f.key} label={label(f.key)}>
            <TextInput type="number" inputMode="decimal" min={0} max={100} step={0.5} value={Number(v ?? 0) / 100} onChange={(e) => set(f.key, Math.round((Number(e.target.value) || 0) * 100))} />
          </Field>
        );
      case "money":
        return (
          <Field key={f.key} label={label(f.key)}>
            <MoneyInput cents={Number(v ?? 0)} onChange={(c) => set(f.key, c)} lang={t.lang} />
          </Field>
        );
      case "enum":
        return (
          <Field key={f.key} label={label(f.key)}>
            <Select<string> value={String(v)} onChange={(x) => set(f.key, x)} options={f.options.map((o) => ({ value: o.value, label: o.label in LANG_NAMES || !o.label.includes("_") ? o.label : t(o.label as Key) }))} />
          </Field>
        );
      case "tax":
        return (
          <Field key={f.key} label={label(f.key)}>
            <Select<number> value={Number(v)} onChange={(x) => set(f.key, x)} options={taxes.filter((x) => x.active).map((x) => ({ value: x.id, label: x.name }))} />
          </Field>
        );
      case "time": {
        const m = Number(v ?? 0);
        const hhmm = `${String(Math.floor(m / 60)).padStart(2, "0")}:${String(m % 60).padStart(2, "0")}`;
        return (
          <Field key={f.key} label={label(f.key)}>
            <TextInput
              type="time"
              dir="ltr"
              value={hhmm}
              onChange={(e) => {
                const [h, mi] = e.target.value.split(":").map(Number);
                if (Number.isFinite(h) && Number.isFinite(mi)) set(f.key, h * 60 + mi);
              }}
            />
          </Field>
        );
      }
      case "logo":
        return (
          <Field key={f.key} label={label(f.key)}>
            <div className="flex items-center gap-3">
              {v ? <img src={String(v)} alt="" className="h-16 w-16 rounded-lg bg-white object-contain" /> : null}
              <label className="panel-2 border line flex min-h-12 cursor-pointer items-center rounded-xl px-4">
                {t("choose_image")}
                <input
                  type="file"
                  accept="image/png,image/jpeg"
                  className="hidden"
                  onChange={(e) => {
                    const file = e.target.files?.[0];
                    if (!file) return;
                    shrinkImage(file, 384).then((url) => set(f.key, url));
                  }}
                />
              </label>
              {v ? (
                <Button variant="plain" onClick={() => set(f.key, "")}>
                  {t("remove")}
                </Button>
              ) : null}
            </div>
          </Field>
        );
    }
  };

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 p-4 pb-28">
      <fieldset disabled={!canEdit} className="contents">
        {SECTIONS.map((s) => (
          <Card key={s.title} title={t(s.title)}>
            <div className="flex flex-col gap-3">{s.fields.map(render)}</div>
          </Card>
        ))}
      </fieldset>
      {canEdit && (
        <div className="fixed bottom-0 end-0 start-0 flex justify-end gap-3 border-t line panel p-3 md:start-56">
          <Button disabled={!dirty} onClick={() => setDraft(saved)}>
            {t("cancel")}
          </Button>
          <Button variant="primary" disabled={!dirty} onClick={save}>
            {t("save")}
          </Button>
        </div>
      )}
      {toast.node}
    </div>
  );
}

/** (moved) */
async function shrinkImage(file: File, max: number): Promise<string> {
  const bmp = await createImageBitmap(file);
  const scale = Math.min(1, max / Math.max(bmp.width, bmp.height));
  const c = document.createElement("canvas");
  c.width = Math.round(bmp.width * scale);
  c.height = Math.round(bmp.height * scale);
  c.getContext("2d")!.drawImage(bmp, 0, 0, c.width, c.height);
  return c.toDataURL("image/png");
}
