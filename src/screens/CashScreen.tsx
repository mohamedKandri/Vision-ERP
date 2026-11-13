import { useCallback, useEffect, useState } from "react";
import { Button, Card, Field, Modal, MoneyField, TextInput, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { formatMoney, formatRate } from "../lib/money";
import { rpc, type SessionUser } from "../lib/rpc";
import { getRegister } from "./PosScreen";

export interface CashReport {
  session_id: number;
  register: string;
  opened_by: string;
  opened_at: string;
  closed_at: string | null;
  z_number: number | null;
  float_cents: number;
  cash_sales_cents: number;
  card_sales_cents: number;
  cash_refunds_cents: number;
  card_refunds_cents: number;
  cash_in_cents: number;
  cash_out_cents: number;
  movements: { kind: "in" | "out"; amount_cents: number; reason: string; user: string; at: string }[];
  expected_cash_cents: number;
  counted_cash_cents: number | null;
  difference_cents: number | null;
  orders: number;
  sales_total_cents: number;
  average_ticket_cents: number;
  discounts_cents: number;
  comps_cents: number;
  voids: number;
  voids_total_cents: number;
  refunds: number;
  by_type: { type: string; count: number; total_cents: number }[];
  vat: { tax_id: number; rate_bp: number; base: number; tax: number; total: number }[];
  open_orders: number;
  net_sales_cents: number;
}

/** X/Z report body, also the printable layout for Phase 4. */
export function ReportView({ r }: { r: CashReport }) {
  const t = useT();
  const m = (c: number) => formatMoney(c, t.lang);
  const dt = (iso: string) => new Date(iso).toLocaleString(t.lang === "ar" ? "ar-MA-u-nu-latn" : t.lang);
  const Row = ({ k, v, bold, warn }: { k: string; v: string; bold?: boolean; warn?: boolean }) => (
    <div className={`flex justify-between gap-3 py-1 ${bold ? "font-bold" : ""} ${warn ? "text-red-500" : ""}`}>
      <span>{k}</span>
      <span dir="ltr">{v}</span>
    </div>
  );
  return (
    <div className="flex flex-col gap-4">
      <div>
        <div className="text-lg font-bold">{r.z_number ? t("cash_z_report", { n: r.z_number }) : t("cash_x_report")} · {r.register}</div>
        <div className="muted text-sm">{t("cash_opened", { u: r.opened_by, d: dt(r.opened_at) })}{r.closed_at ? ` → ${dt(r.closed_at)}` : ""}</div>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <div className="panel-2 rounded-xl border line p-3">
          <Row k={t("cash_float")} v={m(r.float_cents)} />
          <Row k={t("cash_cash_sales")} v={m(r.cash_sales_cents)} />
          <Row k={`${t("cash_refunds")} (${t("rc_cash")})`} v={`-${m(r.cash_refunds_cents)}`} />
          <Row k={t("cash_in")} v={m(r.cash_in_cents)} />
          <Row k={t("cash_out")} v={`-${m(r.cash_out_cents)}`} />
          <Row k={t("cash_expected")} v={m(r.expected_cash_cents)} bold />
          {r.counted_cash_cents !== null && <Row k={t("cash_counted")} v={m(r.counted_cash_cents)} bold />}
          {r.difference_cents !== null && <Row k={t("cash_difference")} v={m(r.difference_cents)} bold warn={r.difference_cents !== 0} />}
        </div>
        <div className="panel-2 rounded-xl border line p-3">
          <Row k={t("cash_cash_sales")} v={m(r.cash_sales_cents)} />
          <Row k={t("cash_card_sales")} v={m(r.card_sales_cents)} />
          <Row k={`${t("cash_refunds")} (${r.refunds})`} v={`-${m(r.cash_refunds_cents + r.card_refunds_cents)}`} />
          <Row k={t("cash_net_sales")} v={m(r.net_sales_cents)} bold />
          <Row k={t("cash_orders")} v={String(r.orders)} />
          <Row k={t("cash_avg_ticket")} v={m(r.average_ticket_cents)} />
          <Row k={t("cash_discounts")} v={m(r.discounts_cents)} />
          <Row k={t("cash_comps")} v={m(r.comps_cents)} />
          <Row k={`${t("cash_voids")} (${r.voids})`} v={m(r.voids_total_cents)} />
        </div>
      </div>
      <div className="grid gap-4 md:grid-cols-2">
        <div className="panel-2 rounded-xl border line p-3">
          <div className="mb-1 font-semibold">{t("cash_by_type")}</div>
          {r.by_type.map((b) => <Row key={b.type} k={`${t(`order_type_${b.type}` as Key)} (${b.count})`} v={m(b.total_cents)} />)}
        </div>
        <div className="panel-2 rounded-xl border line p-3">
          <div className="mb-1 font-semibold">{t("rc_vat")}</div>
          {r.vat.map((b) => <Row key={b.tax_id} k={`${formatRate(b.rate_bp)} · ${t("rc_base")} ${m(b.base)}`} v={m(b.tax)} />)}
        </div>
      </div>
      {r.movements.length > 0 && (
        <div className="panel-2 rounded-xl border line p-3">
          <div className="mb-1 font-semibold">{t("cash_movements")}</div>
          {r.movements.map((mv, i) => <Row key={i} k={`${mv.kind === "in" ? "+" : "−"} ${mv.reason} · ${mv.user}`} v={m(mv.amount_cents)} />)}
        </div>
      )}
    </div>
  );
}

interface SessionRow { id: number; register: string; z_number: number | null; opened_at: string; closed_at: string | null; expected_cents: number | null; counted_cents: number | null; difference_cents: number | null; opened_by: string }

export function CashScreen({ me }: { me: SessionUser }) {
  const t = useT();
  const toast = useToast();
  const register = getRegister();
  const [cur, setCur] = useState<CashReport | null | undefined>(undefined);
  const [history, setHistory] = useState<SessionRow[]>([]);
  const [dialog, setDialog] = useState<null | "open" | "in" | "out" | "close" | { z: CashReport }>(null);
  const [amount, setAmount] = useState<number | null>(null);
  const [reason, setReason] = useState("");
  const canSession = me.permissions.includes("cash.session");

  const load = useCallback(async () => {
    try {
      setCur(await rpc<CashReport | null>("cash.current", { register }));
      if (canSession) setHistory(await rpc<SessionRow[]>("cash.sessions"));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [register, canSession]);
  useEffect(() => void load(), [load]);

  const act = async (fn: () => Promise<unknown>) => {
    try {
      await fn();
      setDialog(null);
      setAmount(null);
      setReason("");
      await load();
      toast.ok(t("saved"));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  if (cur === undefined) return <p className="muted p-6">{t("loading")}</p>;
  const m = (c: number) => formatMoney(c, t.lang);

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 p-4">
      <Card
        title={`${t("nav_cash")} · ${t("cash_register")} ${register}`}
        actions={
          <div className="flex flex-wrap gap-2">
            {!cur && canSession && <Button variant="primary" onClick={() => { setAmount(50000); setDialog("open"); }}>{t("cash_open")}</Button>}
            {cur && me.permissions.includes("cash.in_out") && (
              <>
                <Button onClick={() => setDialog("in")}>{t("cash_in")}</Button>
                <Button onClick={() => setDialog("out")}>{t("cash_out")}</Button>
              </>
            )}
            {cur && canSession && <Button variant="danger" onClick={() => { setAmount(null); setDialog("close"); }}>{t("cash_close")}</Button>}
          </div>
        }
      >
        {cur ? <ReportView r={cur} /> : <p className="muted">{t("cash_none")}</p>}
      </Card>

      {canSession && history.length > 0 && (
        <Card title={t("cash_history")}>
          <div className="flex flex-col divide-y line">
            {history.filter((h) => h.closed_at).map((h) => (
              <div key={h.id} className="flex flex-wrap items-center gap-3 py-2">
                <span className="w-16 font-bold">Z {h.z_number}</span>
                <span className="muted flex-1 text-sm">{h.register} · {new Date(h.closed_at!).toLocaleString()} · {h.opened_by}</span>
                <span className={`font-semibold ${h.difference_cents ? "text-red-500" : "text-emerald-500"}`} dir="ltr">{m(h.difference_cents ?? 0)}</span>
                <Button onClick={async () => setDialog({ z: await rpc<CashReport>("cash.report", { id: h.id }) })}>{t("cash_view")}</Button>
              </div>
            ))}
          </div>
        </Card>
      )}

      {dialog === "open" && (
        <Modal onClose={() => setDialog(null)}>
          <Field label={t("cash_float")}><MoneyField cents={amount} onChange={setAmount} /></Field>
          <div className="mt-4 flex justify-end gap-3">
            <Button onClick={() => setDialog(null)}>{t("cancel")}</Button>
            <Button variant="primary" onClick={() => act(() => rpc("cash.open", { register, float_cents: amount ?? 0 }))}>{t("cash_open")}</Button>
          </div>
        </Modal>
      )}
      {(dialog === "in" || dialog === "out") && (
        <Modal onClose={() => setDialog(null)}>
          <h2 className="mb-3 text-xl font-bold">{t(dialog === "in" ? "cash_in" : "cash_out")}</h2>
          <div className="flex flex-col gap-3">
            <Field label={t("cash_amount")}><MoneyField cents={amount} onChange={setAmount} /></Field>
            <Field label={t("pos_reason")}><TextInput value={reason} onChange={(e) => setReason(e.target.value)} placeholder={dialog === "out" ? "Achat pain, livreur…" : "Monnaie…"} /></Field>
          </div>
          <div className="mt-4 flex justify-end gap-3">
            <Button onClick={() => setDialog(null)}>{t("cancel")}</Button>
            <Button variant="primary" disabled={!amount || !reason.trim()} onClick={() => act(() => rpc("cash.move", { register, kind: dialog, amount_cents: amount, reason }))}>{t("save")}</Button>
          </div>
        </Modal>
      )}
      {dialog === "close" && cur && (
        <Modal onClose={() => setDialog(null)}>
          <h2 className="mb-3 text-xl font-bold">{t("cash_close")}</h2>
          {cur.open_orders > 0 && <p className="mb-3 rounded-xl bg-amber-500/20 p-3 text-amber-600 dark:text-amber-300">{t("cash_open_orders_warning", { n: cur.open_orders })}</p>}
          <Field label={t("cash_counted")}><MoneyField cents={amount} onChange={setAmount} /></Field>
          {amount !== null && (
            <p className={`mt-3 text-lg font-bold ${amount - cur.expected_cash_cents ? "text-red-500" : "text-emerald-500"}`}>
              {t("cash_difference")}: <span dir="ltr">{m(amount - cur.expected_cash_cents)}</span>
            </p>
          )}
          <div className="mt-4 flex justify-end gap-3">
            <Button onClick={() => setDialog(null)}>{t("cancel")}</Button>
            <Button variant="danger" disabled={amount === null}
              onClick={async () => {
                try {
                  const z = await rpc<CashReport>("cash.close", { register, counted_cents: amount });
                  await load();
                  setDialog({ z });
                } catch (e) {
                  toast.error(errorText(t.lang, e));
                }
              }}>
              {t("cash_close")}
            </Button>
          </div>
        </Modal>
      )}
      {dialog && typeof dialog === "object" && (
        <Modal onClose={() => setDialog(null)} wide>
          <ReportView r={dialog.z} />
          <div className="mt-4 flex justify-end"><Button onClick={() => setDialog(null)}>{t("close")}</Button></div>
        </Modal>
      )}
      {toast.node}
    </div>
  );
}
