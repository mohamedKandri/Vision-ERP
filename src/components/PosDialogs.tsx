import QRCode from "qrcode";
import { useEffect, useRef, useState, type ReactNode } from "react";
import { errorText, useT } from "../i18n";
import { formatMoney, parseMoney } from "../lib/money";
import { planPayment, quickCash } from "../lib/order";
import type { ReceiptLine } from "../lib/receipt";
import { isApiError } from "../lib/rpc";
import { PinPad } from "./PinPad";
import { Button, Modal, TextInput } from "./ui";

// ------------------------------------------------------------------ manager approval

type Pending = { run: (pin: string) => Promise<unknown>; resolve: (v: unknown) => void; reject: (e: unknown) => void };

/**
 * Wrap a call that may need a manager. The first attempt runs without a PIN; if the Host
 * answers `approval_required`, a PIN pad opens and the call is retried with the PIN.
 */
export function useApproval() {
  const t = useT();
  const [pending, setPending] = useState<Pending | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState(false);

  async function ask<T>(call: (pin?: string) => Promise<T>): Promise<T> {
    try {
      return await call(undefined);
    } catch (e) {
      if (!isApiError(e) || e.code !== "approval_required") throw e;
      return new Promise<T>((resolve, reject) => {
        setErr(null);
        setPending({ run: (pin) => call(pin), resolve: resolve as (v: unknown) => void, reject });
      });
    }
  }

  const node = pending ? (
    <Modal
      onClose={() => {
        pending.reject({ code: "cancelled", message: "cancelled" });
        setPending(null);
      }}
    >
      <h2 className="text-xl font-bold">{t("pos_manager_pin")}</h2>
      <p className="muted mb-4 text-sm">{t("pos_manager_pin_hint")}</p>
      <PinPad
        key={attempt}
        busy={busy}
        error={err}
        onSubmit={async (pin) => {
          setBusy(true);
          try {
            const v = await pending.run(pin);
            pending.resolve(v);
            setPending(null);
          } catch (e) {
            if (isApiError(e) && (e.code === "wrong_pin" || e.code === "pin_format" || e.code === "locked")) {
              setErr(errorText(t.lang, e));
              setAttempt((a) => a + 1);
            } else {
              pending.reject(e);
              setPending(null);
            }
          } finally {
            setBusy(false);
          }
        }}
      />
    </Modal>
  ) : null;
  return { ask, node };
}

// ------------------------------------------------------------------ reason prompt

export function ReasonDialog({
  title,
  presets,
  onCancel,
  onConfirm,
  danger,
}: {
  title: string;
  presets: string[];
  onCancel: () => void;
  onConfirm: (reason: string) => void;
  danger?: boolean;
}) {
  const t = useT();
  const [reason, setReason] = useState("");
  return (
    <Modal onClose={onCancel}>
      <h2 className="mb-3 text-xl font-bold">{title}</h2>
      <div className="mb-3 flex flex-wrap gap-2">
        {presets.map((p) => (
          <button key={p} onClick={() => setReason(p)} className={`min-h-12 rounded-xl border px-3 ${reason === p ? "border-brand bg-brand/20" : "line panel-2"}`}>
            {p}
          </button>
        ))}
      </div>
      <TextInput placeholder={t("pos_reason")} value={reason} onChange={(e) => setReason(e.target.value)} />
      <div className="mt-4 flex justify-end gap-3">
        <Button onClick={onCancel}>{t("cancel")}</Button>
        <Button variant={danger ? "danger" : "primary"} disabled={!reason.trim()} onClick={() => onConfirm(reason.trim())}>
          {t("pos_apply")}
        </Button>
      </div>
    </Modal>
  );
}

// ------------------------------------------------------------------ receipt preview

/** Paper-like rendering of a receipt. 58 mm ≈ 32 chars, 80 mm ≈ 48 chars. */
export function ReceiptView({ lines, width }: { lines: ReceiptLine[]; width: 32 | 48 }) {
  return (
    <div
      className="mx-auto rounded bg-white px-4 py-5 font-mono text-[13px] leading-snug text-black shadow-inner"
      style={{ width: width === 32 ? "58mm" : "80mm", maxWidth: "100%" }}
      dir="auto"
    >
      {lines.map((l, i) => {
        switch (l.kind) {
          case "logo":
            return <img key={i} src={l.src} alt="" className="mx-auto mb-2 max-h-20 grayscale" />;
          case "rule":
            return <div key={i} className="my-1.5 border-t border-dashed border-black" />;
          case "feed":
            return <div key={i} className="h-3" />;
          case "qr":
            return <Qr key={i} data={l.data} />;
          case "row":
            return (
              <div key={i} className={`flex justify-between gap-2 ${l.bold ? "font-bold" : ""} ${l.big ? "text-base" : ""}`}>
                <span className="min-w-0 break-words">{l.left}</span>
                <span className="shrink-0 whitespace-nowrap" dir="ltr">{l.right}</span>
              </div>
            );
          default:
            return (
              <div key={i} className={`whitespace-pre-wrap break-words ${l.bold ? "font-bold" : ""} ${l.big ? "text-lg" : ""}`}
                style={{ textAlign: l.align === "center" ? "center" : l.align === "right" ? "end" : "start" }}>
                {l.text}
              </div>
            );
        }
      })}
    </div>
  );
}

function Qr({ data }: { data: string }) {
  const [src, setSrc] = useState("");
  useEffect(() => void QRCode.toDataURL(data, { margin: 1, width: 160 }).then(setSrc), [data]);
  return src ? <img src={src} alt="QR" className="mx-auto my-2 h-28 w-28" /> : null;
}

// ------------------------------------------------------------------ payment

export interface PaymentResult {
  payments: { method: "cash" | "card"; amount_cents: number; tendered_cents?: number }[];
}

/** Cash with change, card, or both. Amounts are entered with a big keypad. */
export function PaymentDialog({
  due,
  onCancel,
  onPay,
  footer,
}: {
  due: number;
  onCancel: () => void;
  onPay: (r: PaymentResult) => Promise<void>;
  footer?: ReactNode;
}) {
  const t = useT();
  const m = (c: number) => formatMoney(c, t.lang);
  const [card, setCard] = useState(0);
  const [cashText, setCashText] = useState("");
  const [field, setField] = useState<"cash" | "card">("cash");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const tendered = parseMoney(cashText || "0") ?? 0;
  const plan = planPayment(due, card, tendered);
  const cardText = useRef("");

  const press = (k: string) => {
    if (field === "card") {
      const next = k === "⌫" ? cardText.current.slice(0, -1) : (cardText.current + k).slice(0, 9);
      cardText.current = next;
      setCard(Math.min(parseMoney(next || "0") ?? 0, due));
      return;
    }
    setCashText((s) => (k === "⌫" ? s.slice(0, -1) : (s + k).slice(0, 9)));
  };

  const confirm = async () => {
    const payments: PaymentResult["payments"] = [];
    if (card > 0) payments.push({ method: "card", amount_cents: Math.min(card, due) });
    if (plan.cash > 0) payments.push({ method: "cash", amount_cents: plan.cash, tendered_cents: tendered });
    setBusy(true);
    setErr(null);
    try {
      await onPay({ payments });
    } catch (e) {
      setErr(errorText(t.lang, e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal onClose={onCancel} wide>
      <div className="grid gap-5 md:grid-cols-2">
        <div className="flex flex-col gap-3">
          <div className="flex items-baseline justify-between">
            <span className="muted">{t("pos_due")}</span>
            <span className="text-3xl font-bold" dir="ltr">{m(due)}</span>
          </div>
          <button onClick={() => setField("card")} className={`flex min-h-14 items-center justify-between rounded-xl border px-4 ${field === "card" ? "border-brand bg-brand/10" : "line panel-2"}`}>
            <span>{t("pos_card_amount")}</span>
            <span className="text-xl font-semibold" dir="ltr">{m(card)}</span>
          </button>
          <button onClick={() => setField("cash")} className={`flex min-h-14 items-center justify-between rounded-xl border px-4 ${field === "cash" ? "border-brand bg-brand/10" : "line panel-2"}`}>
            <span>{t("pos_tendered")}</span>
            <span className="text-xl font-semibold" dir="ltr">{m(tendered)}</span>
          </button>
          <div className="flex flex-wrap gap-2">
            {quickCash(due - card).map((c) => (
              <button key={c} className="min-h-12 rounded-xl border line panel-2 px-3 font-semibold" dir="ltr"
                onClick={() => { setField("cash"); setCashText((c / 100).toFixed(2)); }}>
                {m(c)}
              </button>
            ))}
            <button className="min-h-12 rounded-xl border line panel-2 px-3 font-semibold"
              onClick={() => { cardText.current = (due / 100).toFixed(2); setCard(due); setCashText(""); }}>
              {t("pos_all_card")}
            </button>
          </div>
          <div className={`rounded-xl p-4 text-center ${plan.ok ? "bg-emerald-600/15" : "bg-red-600/10"}`}>
            {plan.ok ? (
              <>
                <div className="muted text-sm">{t("pos_change")}</div>
                <div className="text-4xl font-bold text-emerald-500" dir="ltr">{m(plan.change)}</div>
              </>
            ) : (
              <>
                <div className="muted text-sm">{t("rc_due")}</div>
                <div className="text-3xl font-bold text-red-500" dir="ltr">{m(plan.remaining)}</div>
              </>
            )}
          </div>
        </div>
        <div className="flex flex-col gap-3">
          <div className="grid grid-cols-3 gap-2" dir="ltr">
            {["7", "8", "9", "4", "5", "6", "1", "2", "3", ",", "0", "⌫"].map((k) => (
              <button key={k} onClick={() => press(k === "," ? "." : k)} className="h-16 rounded-2xl border line panel-2 text-2xl font-semibold active:scale-95">
                {k}
              </button>
            ))}
          </div>
          {err && <p className="text-red-500">{err}</p>}
          <div className="flex gap-3">
            <Button className="flex-1" onClick={onCancel}>{t("cancel")}</Button>
            <Button variant="primary" className="flex-[2] text-lg" disabled={!plan.ok || busy} onClick={confirm}>
              {t("pos_confirm_payment")}
            </Button>
          </div>
          {footer}
        </div>
      </div>
    </Modal>
  );
}
