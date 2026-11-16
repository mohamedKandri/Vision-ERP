import { PackageCheck, Plus, ShoppingBasket, Trash2, Truck } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Card, Field, Modal, MoneyField, Select, Tabs, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { useLive } from "../lib/live";
import { pickName, type Names } from "../lib/menu";
import { formatMoney } from "../lib/money";
import { lineValue, packName, poTotal, remaining, unitCostOfPack, type PoStatus } from "../lib/purchasing";
import { rpc, type SessionUser } from "../lib/rpc";
import { costUnit, formatQty, type Ingredient, type Unit } from "../lib/stock";
import { QtyField } from "./StockScreen";

type Tab = "orders" | "suggest" | "suppliers";

interface SupplierItem { ingredient_id: number; names: Names; unit: Unit; pack_label: string; pack_milli: number; pack_cost_cents: number; preferred: boolean }
interface Supplier { id: number; name: string; phone: string; email: string; address: string; note: string; active: boolean; open_orders: number; items: SupplierItem[] }
interface PoRow { id: number; number: number; status: PoStatus; supplier_name: string; expected_date: string; created_at: string; total_cents: number; received_cents: number; lines: number }
interface PoLine {
  id: number; ingredient_id: number; names: Names; unit: Unit; pack_label: string; pack_milli: number; qty_packs: number; pack_cost_cents: number;
  ordered_milli: number; total_cents: number; received_milli: number; received_cents: number; stock_milli: number;
}
interface PoDoc {
  id: number; number: number; status: PoStatus; supplier_id: number; supplier_name: string; supplier_phone: string; note: string; expected_date: string;
  created_at: string; ordered_at: string | null; closed_at: string | null; created_by: string; lines: PoLine[]; total_cents: number; received_cents: number;
  receipts: { id: number; at: string; note: string; user_name: string; lines: { po_line_id: number; qty_milli: number; value_cents: number }[] }[];
}
interface Suggestion {
  ingredient_id: number; names: Names; unit: Unit; stock_milli: number; min_milli: number; on_order_milli: number; supplier_id: number | null;
  supplier_name: string | null; pack_label: string; pack_milli: number; pack_cost_cents: number; packs: number; total_cents: number;
}

const poNo = (n: number) => `BC-${n}`;
const STATUS_CLS: Record<PoStatus, string> = {
  draft: "bg-stone-500", ordered: "bg-sky-600", partial: "bg-amber-500", closed: "bg-emerald-600", cancelled: "bg-stone-400",
};

function StatusChip({ s }: { s: PoStatus }) {
  const t = useT();
  return <span className={`rounded px-2 py-0.5 text-xs font-medium text-white ${STATUS_CLS[s]}`}>{t(`pu_status_${s}` as Key)}</span>;
}

export function PurchasesScreen({ me }: { me: SessionUser }) {
  const t = useT();
  const buyer = me.permissions.includes("purchasing.manage");
  const [tab, setTab] = useState<Tab>("orders");
  const [open, setOpen] = useState<number | null>(null);
  const tabs: { id: Tab; label: string }[] = buyer
    ? [{ id: "orders", label: t("pu_tab_orders") }, { id: "suggest", label: t("pu_tab_suggest") }, { id: "suppliers", label: t("pu_tab_suppliers") }]
    : [{ id: "orders", label: t("pu_tab_orders") }];
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4 p-4">
      {open === null && tabs.length > 1 && <Tabs value={tab} onChange={setTab} tabs={tabs} />}
      {open !== null ? (
        <PoDetail id={open} buyer={buyer} onClose={() => setOpen(null)} />
      ) : tab === "orders" ? (
        <OrdersTab buyer={buyer} onOpen={setOpen} />
      ) : tab === "suggest" ? (
        <SuggestTab onCreated={(ids) => { setTab("orders"); if (ids.length === 1) setOpen(ids[0]); }} />
      ) : (
        <SuppliersTab />
      )}
    </div>
  );
}

// ---------------------------------------------------------------- orders list

function OrdersTab({ buyer, onOpen }: { buyer: boolean; onOpen: (id: number) => void }) {
  const t = useT();
  const [filter, setFilter] = useState<"open" | "all">("open");
  const [rows, setRows] = useState<PoRow[] | null>(null);
  const [creating, setCreating] = useState(false);
  const load = useCallback(() => rpc<PoRow[]>("purchases.list", { status: filter === "open" ? "open" : null }).then(setRows, () => {}), [filter]);
  useEffect(() => void load(), [load]);
  useLive(["stock"], load);
  const fmt = new Intl.DateTimeFormat(t.lang === "ar" ? "ar-MA-u-nu-latn" : t.lang, { dateStyle: "short" });
  return (
    <>
      <Card
        title={t("pu_tab_orders")}
        actions={buyer && <Button variant="primary" onClick={() => setCreating(true)}><Plus className="inline h-4 w-4" /> {t("pu_new")}</Button>}
      >
        <div className="mb-3 w-72"><Tabs value={filter} onChange={setFilter} tabs={[{ id: "open", label: t("pu_open") }, { id: "all", label: t("pu_all") }]} /></div>
        {!rows ? (
          <p className="muted">{t("loading")}</p>
        ) : (
          <div className="flex flex-col divide-y line">
            {rows.map((r) => (
              <button key={r.id} onClick={() => onOpen(r.id)} className="grid min-h-14 grid-cols-[5rem_1fr_auto_auto] items-center gap-3 px-1 text-start">
                <span className="num font-semibold">{poNo(r.number)}</span>
                <span className="min-w-0">
                  <span className="block truncate font-medium">{r.supplier_name}</span>
                  <span className="muted text-xs">
                    {fmt.format(new Date(r.created_at))} · {t("pu_lines", { n: r.lines })}
                    {r.expected_date && ` · ${t("pu_expected")} ${r.expected_date}`}
                  </span>
                </span>
                <span className="num text-end text-sm">
                  {formatMoney(r.total_cents, t.lang)}
                  {r.status === "partial" && <span className="muted block text-xs">{t("pu_received")} {formatMoney(r.received_cents, t.lang)}</span>}
                </span>
                <StatusChip s={r.status} />
              </button>
            ))}
            {!rows.length && <p className="muted py-6 text-center">{t("pu_none")}</p>}
          </div>
        )}
      </Card>
      {creating && <PoEditor onClose={() => setCreating(false)} onSaved={(id) => { setCreating(false); onOpen(id); }} />}
    </>
  );
}

// ---------------------------------------------------------------- editor (draft)

type EditLine = { key: number; ingredient_id: number; pack_label: string; pack_milli: number | null; qty_packs: number; pack_cost_cents: number };

function PoEditor({ doc, onClose, onSaved }: { doc?: PoDoc; onClose: () => void; onSaved: (id: number) => void }) {
  const t = useT();
  const [suppliers, setSuppliers] = useState<Supplier[] | null>(null);
  const [ingredients, setIngredients] = useState<Ingredient[]>([]);
  const [supplierId, setSupplierId] = useState<number>(doc?.supplier_id ?? 0);
  const [expected, setExpected] = useState(doc?.expected_date ?? "");
  const [note, setNote] = useState(doc?.note ?? "");
  const [lines, setLines] = useState<EditLine[]>(
    doc?.lines.map((l, k) => ({ key: k, ingredient_id: l.ingredient_id, pack_label: l.pack_label, pack_milli: l.pack_milli, qty_packs: l.qty_packs, pack_cost_cents: l.pack_cost_cents })) ?? [],
  );
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    rpc<Supplier[]>("suppliers.list").then((s) => {
      setSuppliers(s);
      if (!doc && s.find((x) => x.active)) setSupplierId(s.find((x) => x.active)!.id);
    }, () => {});
    rpc<Ingredient[]>("ingredients.list").then(setIngredients, () => {});
  }, [doc]);
  const supplier = suppliers?.find((s) => s.id === supplierId);
  const ingMap = useMemo(() => new Map(ingredients.map((i) => [i.id, i])), [ingredients]);
  // The supplier's items first, then every other active ingredient.
  const options = useMemo(() => {
    const mine = new Set(supplier?.items.map((i) => i.ingredient_id));
    const sorted = [...ingredients.filter((i) => i.active && mine.has(i.id)), ...ingredients.filter((i) => i.active && !mine.has(i.id))];
    return sorted.map((i) => ({ value: i.id, label: pickName(i.names, t.lang) + (mine.has(i.id) ? " ★" : "") }));
  }, [ingredients, supplier, t.lang]);
  const fromSupplier = (ingredient_id: number): Omit<EditLine, "key" | "qty_packs"> => {
    const it = supplier?.items.find((i) => i.ingredient_id === ingredient_id);
    const ing = ingMap.get(ingredient_id);
    return it
      ? { ingredient_id, pack_label: it.pack_label, pack_milli: it.pack_milli, pack_cost_cents: it.pack_cost_cents }
      : { ingredient_id, pack_label: "", pack_milli: ing?.unit === "piece" ? 1000 : 1_000_000, pack_cost_cents: ing?.cost_cents ?? 0 }; // 1 kg / 1 L / 1 piece at the current cost
  };
  const addLine = (ingredient_id?: number) => {
    const id = ingredient_id ?? (options.find((o) => !lines.some((l) => l.ingredient_id === o.value))?.value ?? options[0]?.value);
    if (id) setLines([...lines, { key: Date.now() + Math.random(), qty_packs: 1, ...fromSupplier(id) }]);
  };
  const valid = supplierId > 0 && lines.every((l) => l.pack_milli && l.pack_milli > 0 && l.qty_packs >= 1) && /^(\d{4}-\d{2}-\d{2})?$/.test(expected);
  const save = async (thenOrder: boolean) => {
    try {
      const r = await rpc<{ id: number }>("purchases.save", {
        id: doc?.id ?? null, supplier_id: supplierId, note, expected_date: expected,
        lines: lines.map((l) => ({ ingredient_id: l.ingredient_id, pack_label: l.pack_label, pack_milli: l.pack_milli, qty_packs: l.qty_packs, pack_cost_cents: l.pack_cost_cents })),
      });
      if (thenOrder) await rpc("purchases.order", { id: r.id });
      onSaved(r.id);
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  if (!suppliers) return null;
  return (
    <Modal onClose={onClose} wide>
      <h2 className="mb-3 text-lg font-semibold">{doc ? `${t("edit")} ${poNo(doc.number)}` : t("pu_new")}</h2>
      {!suppliers.some((s) => s.active) ? (
        <p className="muted">{t("pu_no_supplier")}</p>
      ) : (
        <div className="flex flex-col gap-3">
          <div className="grid gap-3 sm:grid-cols-3">
            <Field label={t("pu_supplier")}>
              <Select<number> value={supplierId} onChange={setSupplierId} options={suppliers.filter((s) => s.active || s.id === supplierId).map((s) => ({ value: s.id, label: s.name }))} />
            </Field>
            <Field label={t("pu_expected")}>
              <TextInput type="date" value={expected} onChange={(e) => setExpected(e.target.value)} />
            </Field>
            <Field label={t("st_note")}>
              <TextInput value={note} onChange={(e) => setNote(e.target.value)} maxLength={500} />
            </Field>
          </div>
          <div className="flex flex-col gap-2">
            {lines.map((l, idx) => {
              const ing = ingMap.get(l.ingredient_id);
              const set = (patch: Partial<EditLine>) => setLines(lines.map((x, j) => (j === idx ? { ...x, ...patch } : x)));
              return (
                <div key={l.key} className="grid items-end gap-2 rounded-xl border line p-2 sm:grid-cols-[1fr_9rem_10rem_6rem_8rem_auto]">
                  <Field label={t("st_ingredient")}>
                    <Select<number> value={l.ingredient_id} onChange={(id) => set({ ...fromSupplier(id) })} options={options} />
                  </Field>
                  <Field label={t("pu_pack")}>
                    <TextInput value={l.pack_label} onChange={(e) => set({ pack_label: e.target.value })} maxLength={60} placeholder={ing && l.pack_milli ? formatQty(l.pack_milli, ing.unit, t.lang) : ""} />
                  </Field>
                  <Field label={t("pu_pack_size")}>
                    {ing && <QtyField unit={ing.unit} milli={l.pack_milli} onChange={(m) => set({ pack_milli: m })} />}
                  </Field>
                  <Field label={t("pu_packs")}>
                    <TextInput inputMode="numeric" value={String(l.qty_packs)} onChange={(e) => set({ qty_packs: Math.min(100000, Math.max(0, parseInt(e.target.value.replace(/\D/g, "") || "0", 10))) })} />
                  </Field>
                  <Field label={t("pu_pack_price")}>
                    <MoneyField cents={l.pack_cost_cents} onChange={(c) => set({ pack_cost_cents: c ?? 0 })} />
                  </Field>
                  <button className="mb-0.5 inline-flex h-12 w-12 items-center justify-center rounded-lg border line panel-2" aria-label={t("st_delete")} onClick={() => setLines(lines.filter((_, j) => j !== idx))}>
                    <Trash2 className="h-4 w-4" />
                  </button>
                </div>
              );
            })}
            <Button onClick={() => addLine()} className="self-start" disabled={!options.length}><Plus className="inline h-4 w-4" /> {t("pu_add_line")}</Button>
          </div>
          <p className="text-end text-lg">{t("pu_total")}: <b className="num">{formatMoney(poTotal(lines), t.lang)}</b> <span className="muted text-sm">{t("excl")}</span></p>
          {err && <p className="text-sm text-red-500">{err}</p>}
          <div className="flex flex-wrap justify-end gap-3">
            <Button onClick={onClose}>{t("cancel")}</Button>
            <Button onClick={() => save(false)} disabled={!valid}>{t("pu_save_draft")}</Button>
            <Button variant="primary" onClick={() => save(true)} disabled={!valid || !lines.length}><Truck className="inline h-4 w-4" /> {t("pu_order")}</Button>
          </div>
        </div>
      )}
    </Modal>
  );
}

// ---------------------------------------------------------------- detail + receive

function PoDetail({ id, buyer, onClose }: { id: number; buyer: boolean; onClose: () => void }) {
  const t = useT();
  const toast = useToast();
  const [doc, setDoc] = useState<PoDoc | null>(null);
  const [editing, setEditing] = useState(false);
  const [receiving, setReceiving] = useState(false);
  const [confirm, setConfirm] = useState<"close" | "cancel" | null>(null);
  const load = useCallback(() => rpc<PoDoc>("purchases.get", { id }).then(setDoc, (e) => toast.error(errorText(t.lang, e))), [id]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => void load(), [load]);
  useLive(["stock"], load);
  if (!doc) return <p className="muted">{t("loading")}</p>;
  const fmt = new Intl.DateTimeFormat(t.lang === "ar" ? "ar-MA-u-nu-latn" : t.lang, { dateStyle: "short", timeStyle: "short" });
  const act = async (method: string) => {
    try {
      await rpc(method, { id });
      setConfirm(null);
      load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  const lineName = (lid: number) => { const l = doc.lines.find((x) => x.id === lid); return l ? pickName(l.names, t.lang) : "?"; };
  return (
    <>
      <Card
        title={`${poNo(doc.number)} · ${doc.supplier_name}`}
        actions={<Button onClick={onClose}>{t("back")}</Button>}
      >
        <div className="mb-3 flex flex-wrap items-center gap-3 text-sm">
          <StatusChip s={doc.status} />
          <span className="muted">{t("pu_created", { d: fmt.format(new Date(doc.created_at)), u: doc.created_by })}</span>
          {doc.expected_date && <span>{t("pu_expected")}: <b className="num">{doc.expected_date}</b></span>}
          {doc.supplier_phone && <span className="num">☎ {doc.supplier_phone}</span>}
          {doc.note && <span className="muted">· {doc.note}</span>}
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="muted">
              <tr>
                <th className="py-2 text-start">{t("st_ingredient")}</th>
                <th className="text-end">{t("pu_ordered")}</th>
                <th className="text-end">{t("pu_received")}</th>
                <th className="text-end">{t("pu_pack_price")}</th>
                <th className="text-end">{t("pu_total")}</th>
              </tr>
            </thead>
            <tbody>
              {doc.lines.map((l) => (
                <tr key={l.id} className="border-t line">
                  <td className="py-2 pe-3">
                    <span className="font-medium">{pickName(l.names, t.lang)}</span>
                    <span className="muted block text-xs">{l.qty_packs} × {packName(l.pack_label, l.pack_milli, l.unit, t.lang)}</span>
                  </td>
                  <td className="num text-end whitespace-nowrap">{formatQty(l.ordered_milli, l.unit, t.lang)}</td>
                  <td className={`num text-end whitespace-nowrap ${l.received_milli >= l.ordered_milli ? "text-emerald-600" : l.received_milli > 0 ? "text-amber-600" : "muted"}`}>
                    {formatQty(l.received_milli, l.unit, t.lang)}
                  </td>
                  <td className="num text-end whitespace-nowrap">{formatMoney(l.pack_cost_cents, t.lang)}</td>
                  <td className="num text-end whitespace-nowrap">{formatMoney(l.total_cents, t.lang)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="mt-3 flex flex-wrap items-center justify-end gap-4 border-t line pt-3">
          {doc.received_cents > 0 && <span className="muted">{t("pu_received")}: <span className="num">{formatMoney(doc.received_cents, t.lang)}</span></span>}
          <span className="text-lg">{t("pu_total")}: <b className="num">{formatMoney(doc.total_cents, t.lang)}</b> <span className="muted text-sm">{t("excl")}</span></span>
        </div>
        <div className="mt-4 flex flex-wrap justify-end gap-3">
          {doc.status === "draft" && buyer && (
            <>
              <Button variant="danger" onClick={() => setConfirm("cancel")}>{t("pu_cancel")}</Button>
              <Button onClick={() => setEditing(true)}>{t("edit")}</Button>
              <Button variant="primary" onClick={() => act("purchases.order")} disabled={!doc.lines.length}><Truck className="inline h-4 w-4" /> {t("pu_order")}</Button>
            </>
          )}
          {(doc.status === "ordered" || doc.status === "partial") && (
            <>
              {buyer && doc.status === "ordered" && <Button variant="danger" onClick={() => setConfirm("cancel")}>{t("pu_cancel")}</Button>}
              {buyer && <Button onClick={() => setConfirm("close")}>{t("pu_close")}</Button>}
              <Button variant="primary" onClick={() => setReceiving(true)}><PackageCheck className="inline h-4 w-4" /> {t("pu_receive")}</Button>
            </>
          )}
        </div>
      </Card>
      {doc.receipts.length > 0 && (
        <Card title={t("pu_receipts")}>
          <div className="flex flex-col divide-y line text-sm">
            {doc.receipts.map((r) => (
              <div key={r.id} className="py-2">
                <div className="flex flex-wrap gap-2">
                  <b className="num">{fmt.format(new Date(r.at))}</b>
                  <span>{r.user_name}</span>
                  {r.note && <span className="muted">· {r.note}</span>}
                  <span className="num ms-auto">{formatMoney(r.lines.reduce((s, x) => s + x.value_cents, 0), t.lang)}</span>
                </div>
                <div className="muted">
                  {r.lines.map((x) => {
                    const l = doc.lines.find((y) => y.id === x.po_line_id);
                    return l ? `${lineName(x.po_line_id)} ${formatQty(x.qty_milli, l.unit, t.lang)}` : "";
                  }).join(" · ")}
                </div>
              </div>
            ))}
          </div>
        </Card>
      )}
      {editing && <PoEditor doc={doc} onClose={() => setEditing(false)} onSaved={() => { setEditing(false); load(); }} />}
      {receiving && <ReceiveDialog doc={doc} onClose={() => setReceiving(false)} onDone={(v) => { setReceiving(false); toast.ok(t("pu_received_ok", { v: formatMoney(v, t.lang) })); load(); }} />}
      {confirm && (
        <Modal onClose={() => setConfirm(null)}>
          <h2 className="mb-2 text-lg font-semibold">{t(confirm === "close" ? "pu_close" : "pu_cancel")}</h2>
          <p className="mb-4">{t(confirm === "close" ? "pu_close_hint" : "pu_cancel_hint")}</p>
          <div className="flex justify-end gap-3">
            <Button onClick={() => setConfirm(null)}>{t("back")}</Button>
            <Button variant={confirm === "cancel" ? "danger" : "primary"} onClick={() => act(confirm === "close" ? "purchases.close" : "purchases.cancel")}>{t("st_confirm")}</Button>
          </div>
        </Modal>
      )}
      {toast.node}
    </>
  );
}

function ReceiveDialog({ doc, onClose, onDone }: { doc: PoDoc; onClose: () => void; onDone: (valueCents: number) => void }) {
  const t = useT();
  const [rows, setRows] = useState(doc.lines.map((l) => ({ line: l, qty: remaining(l.ordered_milli, l.received_milli) as number | null, price: l.pack_cost_cents })));
  const [note, setNote] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const total = rows.reduce((s, r) => s + lineValue(r.qty ?? 0, r.line.pack_milli, r.price), 0);
  const any = rows.some((r) => (r.qty ?? 0) > 0);
  const save = async () => {
    try {
      const r = await rpc<{ value_cents: number }>("purchases.receive", {
        id: doc.id, note,
        lines: rows.filter((r) => (r.qty ?? 0) > 0).map((r) => ({ line_id: r.line.id, qty_milli: r.qty, pack_cost_cents: r.price })),
      });
      onDone(r.value_cents);
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  return (
    <Modal onClose={onClose} wide>
      <h2 className="mb-1 text-lg font-semibold">{t("pu_receive")} · {poNo(doc.number)}</h2>
      <p className="muted mb-3 text-sm">{t("pu_receive_hint")}</p>
      <div className="flex flex-col gap-2">
        {rows.map((r, idx) => {
          const l = r.line;
          const set = (patch: Partial<typeof r>) => setRows(rows.map((x, j) => (j === idx ? { ...x, ...patch } : x)));
          const changed = r.price !== l.pack_cost_cents;
          return (
            <div key={l.id} className="grid items-end gap-2 rounded-xl border line p-2 sm:grid-cols-[1fr_12rem_10rem_7rem]">
              <div>
                <div className="font-medium">{pickName(l.names, t.lang)}</div>
                <div className="muted text-xs">
                  {t("pu_still_expected")}: <span className="num">{formatQty(remaining(l.ordered_milli, l.received_milli), l.unit, t.lang)}</span> · {packName(l.pack_label, l.pack_milli, l.unit, t.lang)}
                </div>
              </div>
              <Field label={t("pu_received_now")}>
                <QtyField unit={l.unit} milli={r.qty} onChange={(m) => set({ qty: m })} placeholder="0" />
              </Field>
              <Field label={t("pu_pack_price")}>
                <MoneyField cents={r.price} onChange={(c) => set({ price: c ?? 0 })} />
              </Field>
              <div className="pb-3 text-end">
                <span className="num block">{formatMoney(lineValue(r.qty ?? 0, l.pack_milli, r.price), t.lang)}</span>
                {changed && <span className="muted block text-xs">{formatMoney(unitCostOfPack(l.unit, l.pack_milli, r.price), t.lang)}/{costUnit(l.unit, t.lang)}</span>}
              </div>
            </div>
          );
        })}
        <Field label={t("pu_delivery_note")}>
          <TextInput value={note} onChange={(e) => setNote(e.target.value)} maxLength={300} placeholder={t("pu_delivery_note_ph")} />
        </Field>
        <p className="text-end text-lg">{t("pu_total")}: <b className="num">{formatMoney(total, t.lang)}</b></p>
        <p className="muted text-xs">{t("pu_cost_hint")}</p>
        {err && <p className="text-sm text-red-500">{err}</p>}
        <div className="flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" onClick={save} disabled={!any}><PackageCheck className="inline h-4 w-4" /> {t("pu_receive_save")}</Button>
        </div>
      </div>
    </Modal>
  );
}

// ---------------------------------------------------------------- suggestions

function SuggestTab({ onCreated }: { onCreated: (ids: number[]) => void }) {
  const t = useT();
  const toast = useToast();
  const [rows, setRows] = useState<(Suggestion & { pick: boolean })[] | null>(null);
  const load = useCallback(() => rpc<Suggestion[]>("purchases.suggestions").then((r) => setRows(r.map((x) => ({ ...x, pick: x.supplier_id !== null })))), []);
  useEffect(() => void load(), [load]);
  if (!rows) return <p className="muted">{t("loading")}</p>;
  const picked = rows.filter((r) => r.pick && r.supplier_id !== null && r.packs > 0);
  const total = picked.reduce((s, r) => s + r.packs * r.pack_cost_cents, 0);
  const create = async () => {
    try {
      const r = await rpc<{ ids: number[] }>("purchases.from_suggestions", { picks: picked.map((p) => ({ ingredient_id: p.ingredient_id, supplier_id: p.supplier_id, packs: p.packs })) });
      toast.ok(t("pu_created_n", { n: r.ids.length }));
      onCreated(r.ids);
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  return (
    <Card title={t("pu_tab_suggest")}>
      <p className="muted mb-3 text-sm">{t("pu_suggest_hint")}</p>
      {!rows.length ? (
        <p className="py-6 text-center">{t("pu_suggest_none")}</p>
      ) : (
        <>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="muted">
                <tr>
                  <th />
                  <th className="py-2 text-start">{t("st_ingredient")}</th>
                  <th className="text-end">{t("st_in_stock")} / {t("st_min")}</th>
                  <th className="text-end">{t("pu_on_order")}</th>
                  <th className="ps-3 text-start">{t("pu_supplier")}</th>
                  <th className="text-end">{t("pu_packs")}</th>
                  <th className="text-end">{t("pu_total")}</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((r, idx) => {
                  const set = (patch: Partial<typeof r>) => setRows(rows.map((x, j) => (j === idx ? { ...x, ...patch } : x)));
                  return (
                    <tr key={r.ingredient_id} className="border-t line">
                      <td className="py-1">
                        <input type="checkbox" className="h-6 w-6" checked={r.pick} disabled={r.supplier_id === null} onChange={(e) => set({ pick: e.target.checked })} aria-label={pickName(r.names, t.lang)} />
                      </td>
                      <td className="pe-3 font-medium">{pickName(r.names, t.lang)}</td>
                      <td className="num text-end whitespace-nowrap"><span className={r.stock_milli <= 0 ? "text-red-500" : "text-amber-600"}>{formatQty(r.stock_milli, r.unit, t.lang)}</span> <span className="muted">/ {formatQty(r.min_milli, r.unit, t.lang)}</span></td>
                      <td className="num muted text-end whitespace-nowrap">{r.on_order_milli ? formatQty(r.on_order_milli, r.unit, t.lang) : "—"}</td>
                      <td className="ps-3">
                        {r.supplier_name ?? <span className="text-red-500">{t("pu_no_supplier_for")}</span>}
                        {r.supplier_id !== null && <span className="muted block text-xs">{packName(r.pack_label, r.pack_milli, r.unit, t.lang)} · {formatMoney(r.pack_cost_cents, t.lang)}</span>}
                      </td>
                      <td className="text-end">
                        {r.supplier_id !== null && (
                          <TextInput className="!w-20 text-end" inputMode="numeric" value={String(r.packs)} onChange={(e) => set({ packs: Math.min(100000, parseInt(e.target.value.replace(/\D/g, "") || "0", 10)) })} aria-label={t("pu_packs")} />
                        )}
                      </td>
                      <td className="num text-end whitespace-nowrap">{r.supplier_id !== null ? formatMoney(r.packs * r.pack_cost_cents, t.lang) : ""}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <div className="mt-4 flex flex-wrap items-center justify-end gap-4">
            <span className="text-lg">{t("pu_total")}: <b className="num">{formatMoney(total, t.lang)}</b></span>
            <Button variant="primary" onClick={create} disabled={!picked.length}><ShoppingBasket className="inline h-4 w-4" /> {t("pu_create_drafts")}</Button>
          </div>
        </>
      )}
      {toast.node}
    </Card>
  );
}

// ---------------------------------------------------------------- suppliers

function SuppliersTab() {
  const t = useT();
  const toast = useToast();
  const [rows, setRows] = useState<Supplier[] | null>(null);
  const [edit, setEdit] = useState<Partial<Supplier> | null>(null);
  const load = useCallback(() => rpc<Supplier[]>("suppliers.list").then(setRows, () => {}), []);
  useEffect(() => void load(), [load]);
  useLive(["stock"], load);
  if (!rows) return <p className="muted">{t("loading")}</p>;
  return (
    <>
      <Card title={t("pu_tab_suppliers")} actions={<Button variant="primary" onClick={() => setEdit({ active: true, items: [] })}><Plus className="inline h-4 w-4" /> {t("add")}</Button>}>
        <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
          {rows.map((s) => (
            <button key={s.id} onClick={() => setEdit(s)} className={`panel-2 flex min-h-24 flex-col items-start gap-1 rounded-xl border line p-3 text-start ${s.active ? "" : "opacity-50"}`}>
              <span className="font-semibold">{s.name}</span>
              {s.phone && <span className="num muted text-sm">☎ {s.phone}</span>}
              <span className="muted text-xs">{t("pu_items", { n: s.items.length })}{s.open_orders > 0 && ` · ${t("pu_open_orders", { n: s.open_orders })}`}</span>
              <span className="muted line-clamp-2 text-xs">{s.items.slice(0, 6).map((i) => pickName(i.names, t.lang)).join(", ")}</span>
            </button>
          ))}
          {!rows.length && <p className="muted">{t("pu_no_supplier")}</p>}
        </div>
      </Card>
      {edit && <SupplierDialog value={edit} onClose={() => setEdit(null)} onSaved={() => { setEdit(null); toast.ok(t("saved")); load(); }} />}
      {toast.node}
    </>
  );
}

type ItemEdit = { key: number; ingredient_id: number; pack_label: string; pack_milli: number | null; pack_cost_cents: number; preferred: boolean };

function SupplierDialog({ value, onClose, onSaved }: { value: Partial<Supplier>; onClose: () => void; onSaved: () => void }) {
  const t = useT();
  const [v, setV] = useState(value);
  const [ingredients, setIngredients] = useState<Ingredient[]>([]);
  const [items, setItems] = useState<ItemEdit[]>((value.items ?? []).map((i, k) => ({ key: k, ...i })));
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => void rpc<Ingredient[]>("ingredients.list").then(setIngredients, () => {}), []);
  const ingMap = new Map(ingredients.map((i) => [i.id, i]));
  const valid = !!v.name?.trim() && items.every((i) => i.pack_milli && i.pack_milli > 0) && new Set(items.map((i) => i.ingredient_id)).size === items.length;
  const save = async () => {
    try {
      await rpc("suppliers.save", {
        id: v.id ?? null, name: v.name ?? "", phone: v.phone ?? "", email: v.email ?? "", address: v.address ?? "", note: v.note ?? "", active: v.active ?? true,
        items: items.map((i) => ({ ingredient_id: i.ingredient_id, pack_label: i.pack_label, pack_milli: i.pack_milli, pack_cost_cents: i.pack_cost_cents, preferred: i.preferred })),
      });
      onSaved();
    } catch (e) {
      setErr(errorText(t.lang, e));
    }
  };
  const add = () => {
    const free = ingredients.find((i) => i.active && !items.some((x) => x.ingredient_id === i.id));
    if (free) setItems([...items, { key: Date.now(), ingredient_id: free.id, pack_label: "", pack_milli: free.unit === "piece" ? 1000 : 1_000_000, pack_cost_cents: free.cost_cents, preferred: false }]);
  };
  return (
    <Modal onClose={onClose} wide>
      <h2 className="mb-3 text-lg font-semibold">{v.id ? v.name : t("pu_new_supplier")}</h2>
      <div className="flex flex-col gap-3">
        <div className="grid gap-3 sm:grid-cols-3">
          <Field label={t("name")}><TextInput value={v.name ?? ""} onChange={(e) => setV({ ...v, name: e.target.value })} maxLength={80} autoFocus /></Field>
          <Field label={t("pu_phone")}><TextInput dir="ltr" inputMode="tel" value={v.phone ?? ""} onChange={(e) => setV({ ...v, phone: e.target.value })} maxLength={30} /></Field>
          <Field label={t("pu_email")}><TextInput dir="ltr" type="email" value={v.email ?? ""} onChange={(e) => setV({ ...v, email: e.target.value })} maxLength={120} /></Field>
        </div>
        <div className="grid gap-3 sm:grid-cols-2">
          <Field label={t("pu_address")}><TextInput value={v.address ?? ""} onChange={(e) => setV({ ...v, address: e.target.value })} maxLength={300} /></Field>
          <Field label={t("st_note")}><TextInput value={v.note ?? ""} onChange={(e) => setV({ ...v, note: e.target.value })} maxLength={500} /></Field>
        </div>
        <Toggle checked={v.active ?? true} onChange={(a) => setV({ ...v, active: a })} label={t("active")} />
        <h3 className="mt-2 font-semibold">{t("pu_supplies")}</h3>
        {items.map((it, idx) => {
          const ing = ingMap.get(it.ingredient_id);
          const set = (patch: Partial<ItemEdit>) => setItems(items.map((x, j) => (j === idx ? { ...x, ...patch } : x)));
          return (
            <div key={it.key} className="grid items-end gap-2 rounded-xl border line p-2 sm:grid-cols-[1fr_9rem_10rem_8rem_auto_auto]">
              <Field label={t("st_ingredient")}>
                <Select<number> value={it.ingredient_id} onChange={(id) => set({ ingredient_id: id })} options={ingredients.filter((i) => i.active || i.id === it.ingredient_id).map((i) => ({ value: i.id, label: pickName(i.names, t.lang) }))} />
              </Field>
              <Field label={t("pu_pack")}><TextInput value={it.pack_label} onChange={(e) => set({ pack_label: e.target.value })} maxLength={60} /></Field>
              <Field label={t("pu_pack_size")}>{ing && <QtyField unit={ing.unit} milli={it.pack_milli} onChange={(m) => set({ pack_milli: m })} />}</Field>
              <Field label={t("pu_pack_price")}><MoneyField cents={it.pack_cost_cents} onChange={(c) => set({ pack_cost_cents: c ?? 0 })} /></Field>
              <label className="mb-3 flex items-center gap-2 text-sm" title={t("pu_preferred_hint")}>
                <input type="checkbox" className="h-6 w-6" checked={it.preferred} onChange={(e) => set({ preferred: e.target.checked })} /> {t("pu_preferred")}
              </label>
              <button className="mb-0.5 inline-flex h-12 w-12 items-center justify-center rounded-lg border line panel-2" aria-label={t("st_delete")} onClick={() => setItems(items.filter((_, j) => j !== idx))}>
                <Trash2 className="h-4 w-4" />
              </button>
            </div>
          );
        })}
        <Button onClick={add} className="self-start" disabled={!ingredients.length}><Plus className="inline h-4 w-4" /> {t("st_add_line")}</Button>
        {err && <p className="text-sm text-red-500">{err}</p>}
        <div className="flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" onClick={save} disabled={!valid}>{t("save")}</Button>
        </div>
      </div>
    </Modal>
  );
}
