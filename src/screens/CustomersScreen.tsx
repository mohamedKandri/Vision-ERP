import { Plus, Search, Star } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button, Card, Field, Modal, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { useLive } from "../lib/live";
import { formatPhone, pointsValue, stampProgress, type Customer, type CustomerDetail } from "../lib/loyalty";
import { formatMoney } from "../lib/money";
import { rpc, type Settings } from "../lib/rpc";

export function CustomersScreen() {
  const t = useT();
  const toast = useToast();
  const [q, setQ] = useState("");
  const [rows, setRows] = useState<Customer[] | null>(null);
  const [open, setOpen] = useState<number | "new" | null>(null);
  const [s, setS] = useState<Settings>({});
  const load = useCallback(() => rpc<Customer[]>("customers.list", { q, limit: 200 }).then(setRows, (e) => toast.error(errorText(t.lang, e))),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [q, t.lang]);
  useEffect(() => {
    const id = setTimeout(() => void load(), 250);
    return () => clearTimeout(id);
  }, [load]);
  useEffect(() => void rpc<Settings>("settings.get").then(setS, () => {}), []);
  useLive(["customers", "orders"], load, 30000);
  const m = (c: number) => formatMoney(c, t.lang);
  const mode = String(s.loyalty_mode ?? "none");
  const needed = Number(s.loyalty_stamps_needed ?? 10);
  const balance = (c: Customer) =>
    mode === "stamps"
      ? (() => { const p = stampProgress(c.stamps, needed); return p.rewards ? t("lo_reward_ready", { n: p.rewards }) : t("lo_stamps_of", { n: p.current, of: needed }); })()
      : mode === "points" ? t("lo_points_worth", { n: c.points, v: m(pointsValue(c.points, Number(s.loyalty_point_value_cents ?? 0))) }) : "";

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-4 p-4">
      {toast.node}
      <Card title={t("nav_customers")} actions={<Button variant="primary" onClick={() => setOpen("new")}><Plus className="inline" /> {t("add")}</Button>}>
        <div className="relative mb-3">
          <Search className="muted pointer-events-none absolute start-3 top-3.5 h-5 w-5" />
          <TextInput className="ps-10" placeholder={t("cu_search")} value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        {mode === "none" && <p className="muted mb-3 text-sm">{t("cu_loyalty_off")}</p>}
        {!rows ? <p className="muted">{t("loading")}</p> : rows.length === 0 ? <p className="muted">{t("cu_none")}</p> : (
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead className="muted">
                <tr>
                  <th className="py-2 text-start">{t("name")}</th>
                  <th className="text-start">{t("pos_customer_phone")}</th>
                  <th className="text-end">{t("rp_orders")}</th>
                  <th className="text-end">{t("cu_spent")}</th>
                  {mode !== "none" && <th className="text-start ps-3">{t("cu_loyalty")}</th>}
                </tr>
              </thead>
              <tbody>
                {rows.map((c) => (
                  <tr key={c.id} className={`cursor-pointer border-t line hover:bg-[var(--sel)] ${c.active ? "" : "opacity-50"}`} onClick={() => setOpen(c.id)}>
                    <td className="py-3 pe-3 font-medium">{c.name || "—"}</td>
                    <td className="num pe-3" dir="ltr">{formatPhone(c.phone)}</td>
                    <td className="num pe-3 text-end">{c.orders}</td>
                    <td className="num pe-3 text-end whitespace-nowrap">{m(c.spent_cents)}</td>
                    {mode !== "none" && <td className="ps-3">{balance(c)}</td>}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </Card>
      {open !== null && (
        <CustomerDialog id={open === "new" ? null : open} mode={mode} balance={balance}
          onClose={() => setOpen(null)} onSaved={(msg) => { toast.ok(msg); void load(); }} onError={(e) => toast.error(errorText(t.lang, e))} />
      )}
    </div>
  );
}

function CustomerDialog({ id, mode, balance, onClose, onSaved, onError }: {
  id: number | null; mode: string; balance: (c: Customer) => string;
  onClose: () => void; onSaved: (msg: string) => void; onError: (e: unknown) => void;
}) {
  const t = useT();
  const m = (c: number) => formatMoney(c, t.lang);
  const [c, setC] = useState<CustomerDetail | null>(null);
  const [f, setF] = useState({ name: "", phone: "", address: "", note: "", active: true });
  const [adj, setAdj] = useState({ amount: "", reason: "" });
  // The parent passes a new onError each render: keep it in a ref so loading runs once per id.
  const err = useRef(onError);
  err.current = onError;
  const reload = useCallback(() => {
    if (id === null) return;
    rpc<CustomerDetail>("customers.get", { id }).then((d) => {
      setC(d);
      setF({ name: d.name, phone: d.phone, address: d.address, note: d.note, active: d.active });
    }, (e) => err.current(e));
  }, [id]);
  useEffect(() => reload(), [reload]);
  const fmt = new Intl.DateTimeFormat(`${t.lang === "ar" ? "ar-MA" : t.lang === "en" ? "en-GB" : "fr-FR"}-u-nu-latn`, { dateStyle: "short", timeStyle: "short" });

  const save = async () => {
    try {
      await rpc("customers.save", { id, ...f });
      onSaved(t("saved"));
      if (id === null) onClose();
      else reload();
    } catch (e) {
      onError(e);
    }
  };
  const adjust = async () => {
    const n = Number(adj.amount);
    if (!Number.isInteger(n) || n === 0 || id === null) return;
    try {
      await rpc("loyalty.adjust", { customer_id: id, stamps: mode === "stamps" ? n : 0, points: mode === "points" ? n : 0, reason: adj.reason });
      setAdj({ amount: "", reason: "" });
      onSaved(t("saved"));
      reload();
    } catch (e) {
      onError(e);
    }
  };
  const moveLabel = (k: string) => t(`cu_move_${k}` as Key);

  return (
    <Modal onClose={onClose} wide>
      <h2 className="mb-4 text-xl font-bold">{id === null ? t("cu_new") : c?.name || formatPhone(c?.phone ?? "")}</h2>
      <div className="grid gap-3 sm:grid-cols-2">
        <Field label={t("pos_customer_phone")}><TextInput inputMode="tel" dir="ltr" value={f.phone} onChange={(e) => setF({ ...f, phone: e.target.value })} autoFocus={id === null} /></Field>
        <Field label={t("name")}><TextInput value={f.name} onChange={(e) => setF({ ...f, name: e.target.value })} /></Field>
        <Field label={t("pos_customer_address")}><TextInput value={f.address} onChange={(e) => setF({ ...f, address: e.target.value })} /></Field>
        <Field label={t("cu_note")}><TextInput value={f.note} onChange={(e) => setF({ ...f, note: e.target.value })} /></Field>
      </div>
      <div className="mt-3 flex flex-wrap items-center justify-between gap-2">
        <Toggle checked={f.active} onChange={(v) => setF({ ...f, active: v })} label={t("active")} />
        <div className="flex gap-2">
          <Button onClick={onClose}>{t("close")}</Button>
          <Button variant="primary" onClick={save} disabled={!f.phone.trim()}>{t("save")}</Button>
        </div>
      </div>

      {c && (
        <>
          <div className="mt-5 grid grid-cols-2 gap-3 sm:grid-cols-3">
            <div className="panel-2 rounded-xl p-3"><div className="muted text-xs">{t("rp_orders")}</div><div className="num text-xl font-semibold">{c.orders}</div></div>
            <div className="panel-2 rounded-xl p-3"><div className="muted text-xs">{t("cu_spent")}</div><div className="num text-xl font-semibold">{m(c.spent_cents)}</div></div>
            {mode !== "none" && (
              <div className="panel-2 col-span-2 rounded-xl p-3 sm:col-span-1">
                <div className="muted flex items-center gap-1 text-xs"><Star className="h-3.5 w-3.5 text-amber-500" />{t("cu_loyalty")}</div>
                <div className="text-lg font-semibold">{balance(c)}</div>
              </div>
            )}
          </div>

          {mode !== "none" && (
            <div className="mt-4 grid gap-2 sm:grid-cols-[140px_1fr_auto] sm:items-end">
              <Field label={mode === "stamps" ? t("cu_adjust_stamps") : t("cu_adjust_points")}>
                <TextInput inputMode="numeric" dir="ltr" placeholder="+1 / -1" value={adj.amount} onChange={(e) => setAdj({ ...adj, amount: e.target.value.replace(/[^\d-]/g, "") })} />
              </Field>
              <Field label={t("rp_reason")}><TextInput value={adj.reason} onChange={(e) => setAdj({ ...adj, reason: e.target.value })} /></Field>
              <Button onClick={adjust} disabled={!adj.amount || !adj.reason.trim()}>{t("cu_adjust")}</Button>
            </div>
          )}

          <h3 className="mb-2 mt-5 font-semibold">{t("cu_history")}</h3>
          {c.order_list.length === 0 ? <p className="muted text-sm">{t("cu_no_orders")}</p> : (
            <div className="max-h-56 overflow-auto">
              <table className="w-full text-sm">
                <tbody>
                  {c.order_list.map((o) => (
                    <tr key={o.id} className="border-t line">
                      <td className="num py-2 pe-3">#{o.number}{o.split_index ? `-${o.split_index}` : ""}</td>
                      <td className="pe-3">{fmt.format(new Date(o.created_at))}</td>
                      <td className="pe-3">{t(`order_type_${o.type}` as Key)}</td>
                      <td className="muted pe-3">{t(`pay_${o.payment_status}` as Key)}</td>
                      <td className="num text-end whitespace-nowrap">{m(o.total_cents)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}

          {mode !== "none" && c.moves.length > 0 && (
            <>
              <h3 className="mb-2 mt-5 font-semibold">{t("cu_moves")}</h3>
              <div className="max-h-48 overflow-auto">
                <table className="w-full text-sm">
                  <tbody>
                    {c.moves.map((mv, i) => (
                      <tr key={i} className="border-t line">
                        <td className="py-2 pe-3">{fmt.format(new Date(mv.at))}</td>
                        <td className="pe-3">{moveLabel(mv.kind)}{mv.number ? ` · #${mv.number}` : ""}{mv.kind === "adjust" && mv.reason ? ` · ${mv.reason}` : ""}</td>
                        <td className={`num text-end ${mv.stamps + mv.points < 0 ? "text-red-500" : "text-emerald-600"}`} dir="ltr">
                          {mv.stamps ? `${mv.stamps > 0 ? "+" : ""}${mv.stamps} ★` : ""}{mv.points ? `${mv.points > 0 ? "+" : ""}${mv.points} pts` : ""}
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}
        </>
      )}
    </Modal>
  );
}
