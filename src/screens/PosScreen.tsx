import { LayoutDashboard, Bell, Star, ChefHat, ClipboardList, Minus, Pause, Percent, Plus, Receipt, Scissors, Trash2, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import FloorScreen from "./FloorScreen";
import { ItemConfigurator } from "../components/ItemConfigurator";
import { PaymentDialog, ReasonDialog, ReceiptView, useApproval } from "../components/PosDialogs";
import { Button, Field, Modal, MoneyField, TextInput, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { defaultConfig, describe, MenuIndex, pickName, unitPrice, type ItemConfig, type Menu, type Product } from "../lib/menu";
import { computeOrder, formatMoney, formatRate, type Discount, type LineInput, type Tax } from "../lib/money";
import { orderLabel, type Order, type OrderLine, type OrderType } from "../lib/order";
import { buildReceipt } from "../lib/receipt";
import { isApiError, rpc, type Lang, type SessionUser, type Settings, type TaxRow } from "../lib/rpc";
import { device } from "../lib/device";
import { useLive } from "../lib/live";
import { formatPhone, normalizePhone, pointsValue, stampProgress, type Customer } from "../lib/loyalty";
import { FloorBoard, FloorTabs, StateLegend, usePlan, type PlanTable } from "../components/FloorPlan";
import { PrintProblems } from "./PrintersScreen";
import { printReceipt, type ReceiptPrinter } from "../lib/print";

interface Ctx {
  menu: Menu;
  settings: Settings;
  tables: { id: number; name: string; seats: number }[];
  taxes: TaxRow[];
  happy_hour: boolean;
  cash_session: { session_id: number } | null;
  receipt_printer: ReceiptPrinter | null;
}

interface CartLine {
  key: number;
  config: ItemConfig;
  qty: number;
  note: string;
}

interface Draft {
  type: OrderType;
  table_id: number | null;
  customer_name: string;
  customer_phone: string;
  customer_address: string;
  delivery_fee_cents: number | null;
}

/** This device's register (Settings → Device). */
export const getRegister = () => device()?.register || "main";
const getPaper = (): 32 | 48 => {
  try {
    return localStorage.getItem("burger.paper") === "58" ? 32 : 48;
  } catch {
    return 48;
  }
};

function beep() {
  try {
    const ac = new AudioContext();
    const o = ac.createOscillator();
    o.frequency.value = 880;
    o.connect(ac.destination);
    o.start();
    o.stop(ac.currentTime + 0.25);
  } catch {
    /* no audio */
  }
}

export function PosScreen({ me }: { me: SessionUser }) {
  const t = useT();
  const toast = useToast();
  const approval = useApproval();
  const m = (c: number) => formatMoney(c, t.lang);
  const register = getRegister();

  const [ctx, setCtx] = useState<Ctx | null>(null);
  const [cat, setCat] = useState<number | null>(null);
  const [order, setOrder] = useState<Order | null>(null);
  const [cart, setCart] = useState<CartLine[]>([]);
  // Restaurant mode: the till opens on the floor plan; picking a table starts or opens its order.
  const [floorMode, setFloorMode] = useState(true);
  const [draft, setDraft] = useState<Draft>({ type: "takeaway", table_id: null, customer_name: "", customer_phone: "", customer_address: "", delivery_fee_cents: null });
  const [configuring, setConfiguring] = useState<{ productId: number; editKey?: number; initial?: ItemConfig } | null>(null);
  const [sheet, setSheet] = useState<{ kind: "cart"; key: number } | { kind: "line"; id: number } | null>(null);
  const [dialog, setDialog] = useState<
    | null
    | { kind: "pay" }
    | { kind: "paid"; change: number }
    | { kind: "receipt"; reprint: boolean }
    | { kind: "orders" }
    | { kind: "discount"; lineId?: number }
    | { kind: "hold" }
    | { kind: "void" }
    | { kind: "void_line"; id: number }
    | { kind: "refund" }
    | { kind: "split" }
    | { kind: "tables" }
    | { kind: "open_till" }
  >(null);
  const [readyCount, setReadyCount] = useState(0);
  const keyRef = useRef(1);

  const loadCtx = useCallback(async () => {
    try {
      const c = await rpc<Ctx>("pos.context", { register });
      setCtx(c);
      setCat((x) => x ?? c.menu.categories[0]?.id ?? null);
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [register]);
  useEffect(() => void loadCtx(), [loadCtx]);
  // Menu, 86'd items, prices or the till changed on any device: reload.
  useLive(["menu", "cash", "settings", "print"], loadCtx, 60000);

  // Ready orders: pushed live when the kitchen bumps; beep + toast when a new one is ready.
  const lastReady = useRef<number[]>([]);
  const tickReady = useCallback(() => {
    rpc<{ id: number; number: number; split_index: number }[]>("orders.list", { scope: "ready" }).then((r) => {
      const fresh = r.filter((o) => !lastReady.current.includes(o.id));
      if (fresh.length && lastReady.current.length + fresh.length > 0 && r.length >= lastReady.current.length) {
        beep();
        toast.ok(t("pos_order_ready", { n: orderLabel(fresh[0]) }));
      }
      lastReady.current = r.map((o) => o.id);
      setReadyCount(r.length);
    }, () => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => tickReady(), [tickReady]);
  useLive(["orders"], () => {
    tickReady();
    if (order) rpc<Order>("orders.get", { id: order.id }).then(setOrder, () => {});
  }, 15000);

  const ix = useMemo(() => (ctx ? new MenuIndex(ctx.menu) : null), [ctx]);
  const taxById = useMemo(() => new Map((ctx?.taxes ?? []).map((x) => [x.id, x])), [ctx]);
  const s = ctx?.settings ?? {};
  const receiptLang = (s.default_language as Lang) ?? "fr";

  const fail = (e: unknown) => {
    if (isApiError(e) && e.code === "cancelled") return;
    toast.error(errorText(t.lang, e));
  };

  // -------------------------------------------------------------- estimate (same maths as the Host)
  const estimate = useMemo(() => {
    if (!ix || !ctx) return null;
    const toTax = (id: number): Tax => {
      const x = taxById.get(id);
      return { id, rate_bp: x?.rate_bp ?? 0, price_include: x?.price_include ?? true };
    };
    const lines: LineInput[] = [];
    for (const l of order?.lines.filter((x) => !x.voided) ?? []) {
      const discount: Discount =
        l.discount_kind === "comp" ? { kind: "percent", bp: 10000 } : l.discount_kind === "percent" ? { kind: "percent", bp: l.discount_value ?? 0 } : l.discount_kind === "fixed" ? { kind: "fixed", cents: l.discount_value ?? 0 } : null;
      lines.push({ unit_cents: l.unit_cents, qty: l.qty, tax: { id: l.tax_id, rate_bp: l.tax_rate_bp, price_include: l.price_include }, discount });
    }
    for (const c of cart) {
      const p = ix.product(c.config.product_id);
      lines.push({ unit_cents: unitPrice(ix, c.config, ctx.happy_hour), qty: c.qty, tax: toTax(p.tax_id) });
    }
    const type = order?.type ?? draft.type;
    const fee = type === "delivery" ? (order ? order.delivery_fee_cents : draft.delivery_fee_cents ?? Number(s.delivery_fee_cents ?? 0)) : 0;
    const od: Discount = order?.discount_kind === "percent" ? { kind: "percent", bp: order.discount_value ?? 0 } : order?.discount_kind === "fixed" ? { kind: "fixed", cents: order.discount_value ?? 0 } : null;
    return computeOrder(lines, {
      rounding: s.tax_rounding === "global" ? "global" : "per_line",
      orderDiscount: od,
      extraFee: fee > 0 ? { cents: fee, tax: toTax(Number(s.default_tax_id ?? 1)) } : undefined,
    });
  }, [ix, ctx, order, cart, draft, taxById, s]);

  // Phone typed for a new order: look the customer up and fill in what's empty.
  const [found, setFound] = useState<Customer | null>(null);
  useEffect(() => {
    if (order) return;
    const phone = normalizePhone(draft.customer_phone);
    if (!phone) return setFound(null);
    const id = setTimeout(() => {
      rpc<Customer | null>("customers.lookup", { phone }).then((c) => {
        setFound(c);
        if (c) setDraft((d) => ({ ...d, customer_name: d.customer_name || c.name, customer_address: d.customer_address || c.address }));
      }, () => setFound(null));
    }, 350);
    return () => clearTimeout(id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft.customer_phone, order?.id]);
  if (!ctx || !ix) return <p className="muted p-6">{t("loading")}</p>;

  // -------------------------------------------------------------- actions
  const newOrder = () => {
    setFloorMode(true);
    setOrder(null);
    setCart([]);
    setDraft({ type: "takeaway", table_id: null, customer_name: "", customer_phone: "", customer_address: "", delivery_fee_cents: null });
    setDialog(null);
  };

  const addProduct = (p: Product) => {
    if (!p.available) return;
    const needsChoice = p.kind === "combo" || p.variants.some((v) => v.active) || ix.groupsOf(p).length > 0;
    if (needsChoice) setConfiguring({ productId: p.id });
    else addToCart(defaultConfig(ix, p.id));
  };

  const addToCart = (config: ItemConfig, editKey?: number) => {
    if (editKey !== undefined) {
      setCart((c) => c.map((x) => (x.key === editKey ? { ...x, config } : x)));
      return;
    }
    setCart((c) => {
      const same = c.find((x) => JSON.stringify(x.config) === JSON.stringify(config) && !x.note);
      if (same) return c.map((x) => (x === same ? { ...x, qty: x.qty + 1 } : x));
      return [...c, { key: keyRef.current++, config, qty: 1, note: "" }];
    });
  };

  const submit = async (opts: { send?: boolean; hold?: boolean; hold_label?: string } = {}): Promise<Order> => {
    const o = await rpc<Order & { offline?: boolean }>("orders.submit", {
      order_id: order?.id ?? null,
      type: draft.type,
      table_id: draft.type === "dine_in" ? draft.table_id : null,
      customer_name: draft.customer_name,
      customer_phone: draft.customer_phone,
      customer_address: draft.customer_address,
      delivery_fee_cents: draft.type === "delivery" ? draft.delivery_fee_cents : null,
      lines: cart.map((c) => ({ config: c.config, qty: c.qty, note: c.note })),
      send: !!opts.send,
      hold: !!opts.hold,
      hold_label: opts.hold_label ?? null,
      register,
    });
    if (o.offline) {
      // Kept on this device until the Host is reachable again.
      toast.ok(t("offline_saved"));
      newOrder();
      throw { code: "cancelled", message: "offline" };
    }
    setOrder(o);
    setCart([]);
    return o;
  };

  const send = async () => {
    try {
      const o = await submit({ send: true });
      toast.ok(`#${orderLabel(o)} → ${t("status_sent")}`);
    } catch (e) {
      fail(e);
    }
  };

  const startPay = async () => {
    if (!ctx.cash_session) return setDialog({ kind: "open_till" });
    try {
      if (cart.length || !order) await submit({ send: true });
      setDialog({ kind: "pay" });
    } catch (e) {
      fail(e);
    }
  };

  const load = async (id: number) => {
    try {
      const o = await rpc<Order>("orders.get", { id });
      setOrder(o);
      setCart([]);
      setDraft({ type: o.type, table_id: o.table_id, customer_name: o.customer_name, customer_phone: o.customer_phone, customer_address: o.customer_address, delivery_fee_cents: o.delivery_fee_cents });
      setDialog(null);
    } catch (e) {
      fail(e);
    }
  };

  const call = async (method: string, params: Record<string, unknown>, ok?: string) => {
    try {
      const o = await approval.ask((approver_pin) => rpc<Order>(method, { ...params, approver_pin, register }));
      setOrder(o);
      if (ok) toast.ok(ok);
      return o;
    } catch (e) {
      fail(e);
      return null;
    }
  };

  const loyaltyMode = String(s.loyalty_mode ?? "none");
  const stampsNeeded = Number(s.loyalty_stamps_needed ?? 10);
  const pointValue = Number(s.loyalty_point_value_cents ?? 0);
  const loyaltyText = (stamps: number, points: number) =>
    loyaltyMode === "stamps"
      ? (() => { const p = stampProgress(stamps, stampsNeeded); return p.rewards > 0 ? t("lo_reward_ready", { n: p.rewards }) : t("lo_stamps_of", { n: p.current, of: stampsNeeded }); })()
      : loyaltyMode === "points" ? t("lo_points_worth", { n: points, v: m(pointsValue(points, pointValue)) }) : "";

  const types: OrderType[] = (["dine_in", "takeaway", "delivery"] as const).filter((x) => s[`order_type_${x}`] !== false);
  const locked = !!order;
  const editable = !order || !["closed", "voided"].includes(order.status);
  const canPay = (cart.length > 0 || (order && order.due_cents > 0) || (order && order.total_cents === 0 && order.payment_status !== "paid" && order.lines.some((l) => !l.voided))) && editable;
  const products = ctx.menu.products.filter((p) => p.category_id === cat);
  const lineSel = sheet?.kind === "line" ? order?.lines.find((l) => l.id === sheet.id) : undefined;
  const cartSel = sheet?.kind === "cart" ? cart.find((c) => c.key === sheet.key) : undefined;

  // -------------------------------------------------------------- render
  const totalShown = cart.length ? estimate?.total ?? 0 : order?.total_cents ?? 0;
  const baseShown = cart.length ? estimate?.base ?? 0 : order?.base_cents ?? 0;
  const taxShown = cart.length ? estimate?.tax ?? 0 : order?.tax_cents ?? 0;
  const fkey = "flex min-w-0 flex-1 items-center justify-center gap-2 px-2 text-[13px] font-semibold disabled:opacity-30 [&>svg]:h-4 [&>svg]:w-4 hover:bg-white/5";
  const startOrder = (type: "takeaway" | "delivery" | "dine_in", table_id: number | null = null) => {
    setOrder(null);
    setCart([]);
    setDraft({ type, table_id, customer_name: "", customer_phone: "", customer_address: "", delivery_fee_cents: null });
    setDialog(null);
    setFloorMode(false);
  };

  if (floorMode && ctx.tables.length > 0) {
    return (
      <div className="flex h-full min-h-0 flex-col">
        <PrintProblems />
        <FloorHome
          canEdit={me.permissions.includes("settings.manage")}
          onTakeaway={() => startOrder("takeaway")}
          onDelivery={() => startOrder("delivery")}
          onTable={async (tb) => {
            if (tb.orders.length) {
              await load(tb.orders[tb.orders.length - 1].id);
              setFloorMode(false);
            } else startOrder("dine_in", tb.id);
          }}
        />
        {toast.node}
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-0 flex-col">
      <PrintProblems />
      {!ctx.cash_session && (
        <div className="flex items-center justify-between gap-3 bg-amber-500/20 px-4 py-2 text-amber-700 dark:text-amber-300">
          <span className="font-medium">{t("pos_till_closed")}</span>
          {me.permissions.includes("cash.session") && <Button variant="primary" onClick={() => setDialog({ kind: "open_till" })}>{t("pos_open_till")}</Button>}
        </div>
      )}
      {ctx.happy_hour && <div className="bg-pink-600 px-4 py-1 text-center text-sm font-bold text-white">{t("pos_happy_hour")}</div>}

      <div className="grid min-h-0 flex-1 grid-cols-1 gap-px bg-[var(--line)] lg:grid-cols-[400px_minmax(0,1fr)_280px]">
        {/* ---------------- ticket (left) */}
        <section aria-label={t("pos_receipt")} className="flex min-h-0 flex-col bg-[var(--panel)]">
          <div className="flex items-center gap-2 border-b line px-3 py-2">
            {ctx.tables.length > 0 && (
              <button onClick={() => setFloorMode(true)} title={t("fl_tables")}
                className="flex h-11 shrink-0 items-center gap-1.5 rounded-sm bg-brand px-3 text-[13px] font-semibold text-white">
                <LayoutDashboard className="h-4 w-4" />{t("fl_tables")}
              </button>
            )}
            <div className="min-w-0 flex-1">
              <b className="num text-lg">{order ? `#${orderLabel(order)}` : t("pos_new_order")}</b>
              <span className="muted ms-2 text-[13px]">
                {t(`order_type_${order?.type ?? draft.type}` as Key)}
                {draft.type === "dine_in" && draft.table_id ? ` · ${ctx.tables.find((x) => x.id === draft.table_id)?.name ?? ""}` : ""}
                {draft.customer_name ? ` · ${draft.customer_name}` : ""}
              </span>
              {order && <div className="muted text-xs">{t(`status_${order.status}` as Key)} · {t(`pay_${order.payment_status}` as Key)}</div>}
            </div>
          </div>
          {!locked && (
            <div className="flex flex-col gap-1.5 border-b line p-2">
              <div className="grid grid-cols-3 gap-1">
                {types.map((ty) => (
                  <button key={ty} onClick={() => setDraft({ ...draft, type: ty })}
                    className={`h-10 rounded-sm border text-[13px] font-semibold ${draft.type === ty ? "border-brand bg-brand text-white" : "line bg-[var(--panel)]"}`}>
                    {t(`order_type_${ty}` as Key)}
                  </button>
                ))}
              </div>
              {draft.type === "dine_in" && (
                <button className="h-10 rounded-sm border line px-3 text-start text-[13px]" onClick={() => setDialog({ kind: "tables" })}>
                  {t("rc_table")}: <b>{ctx.tables.find((x) => x.id === draft.table_id)?.name ?? t("pos_no_table")}</b>
                </button>
              )}
              {draft.type !== "dine_in" && (
                <div className="grid grid-cols-2 gap-1">
                  <TextInput className="!min-h-10" inputMode="tel" dir="ltr" placeholder={t("pos_customer_phone")} value={draft.customer_phone} onChange={(e) => setDraft({ ...draft, customer_phone: e.target.value })} />
                  <TextInput className="!min-h-10" placeholder={t("pos_customer_name")} value={draft.customer_name} onChange={(e) => setDraft({ ...draft, customer_name: e.target.value })} />
                </div>
              )}
              {draft.type === "delivery" && (
                <TextInput className="!min-h-10" placeholder={t("pos_customer_address")} value={draft.customer_address} onChange={(e) => setDraft({ ...draft, customer_address: e.target.value })} />
              )}
              {draft.type !== "dine_in" && found && (
                <div className="flex items-center gap-1.5 rounded-sm bg-amber-500/10 px-2 py-1 text-[13px]" data-testid="customer-chip">
                  <Star className="h-3.5 w-3.5 shrink-0 text-amber-500" />
                  <b className="truncate">{found.name || formatPhone(found.phone)}</b>
                  <span className="muted truncate">· {t("lo_n_orders", { n: found.orders })}{loyaltyMode !== "none" ? ` · ${loyaltyText(found.stamps, found.points)}` : ""}</span>
                </div>
              )}
            </div>
          )}
          <div className="label grid grid-cols-[44px_1fr_auto] gap-2 border-b line px-3 py-1.5">
            <span>{t("pos_qty")}</span><span>{t("tab_products")}</span><span>{t("pos_total")}</span>
          </div>
          <div className="min-h-0 flex-1 overflow-auto">
            {!order?.lines.length && !cart.length && <p className="muted p-6 text-center">{t("pos_empty_cart")}</p>}
            {order?.lines.map((l) => (
              <button key={l.id} disabled={!editable} onClick={() => setSheet({ kind: "line", id: l.id })}
                className={`grid w-full grid-cols-[44px_1fr_auto] gap-2 border-b line px-3 py-2 text-start ${l.voided ? "opacity-40 line-through" : ""}`}>
                <span className="num font-bold">{l.qty}</span>
                <span className="min-w-0">
                  <span className="font-semibold">{pickName(l.names, t.lang)}</span>
                  {l.kitchen_status !== "new" && <ChefHat className="ms-1 inline h-3.5 w-3.5 text-emerald-500" aria-label={t("pos_lines_sent")} />}
                  {(l.details[t.lang] ?? l.details.fr).map((d, i) => <span key={i} className="muted block text-xs">{d}</span>)}
                  {l.note && <span className="block text-xs text-amber-600">{t("pos_note")}: {l.note}</span>}
                  {l.discount_kind && <span className="block text-xs text-pink-600">{l.discount_kind === "comp" ? t("pos_comp") : l.discount_kind === "percent" ? `-${formatRate(l.discount_value ?? 0)}` : `-${m(l.discount_value ?? 0)}`}</span>}
                </span>
                <span className="font-bold" dir="ltr">{m(l.voided ? 0 : l.gross_cents)}</span>
              </button>
            ))}
            {cart.length > 0 && order && <div className="label bg-brand/10 px-3 py-1">{t("pos_new_items")}</div>}
            {cart.map((c) => {
              const p = ix.product(c.config.product_id);
              return (
                <div key={c.key} className="sel grid grid-cols-[auto_1fr_auto] items-center gap-2 border-b line px-2 py-1.5">
                  <div className="flex items-center" dir="ltr">
                    <button aria-label="-" className="h-9 w-9 rounded-sm border line bg-[var(--panel)]" onClick={() => setCart((x) => x.flatMap((y) => (y.key === c.key ? (y.qty > 1 ? [{ ...y, qty: y.qty - 1 }] : []) : [y])))}><Minus className="mx-auto h-3.5 w-3.5" /></button>
                    <span className="w-7 text-center font-bold">{c.qty}</span>
                    <button aria-label="+" className="h-9 w-9 rounded-sm border line bg-[var(--panel)]" onClick={() => setCart((x) => x.map((y) => (y.key === c.key ? { ...y, qty: y.qty + 1 } : y)))}><Plus className="mx-auto h-3.5 w-3.5" /></button>
                  </div>
                  <button className="min-w-0 text-start" onClick={() => setSheet({ kind: "cart", key: c.key })}>
                    <span className="font-semibold">{pickName(p.names, t.lang)}</span>
                    {describe(ix, c.config, t.lang).map((d, i) => <span key={i} className="muted block text-xs">{d}</span>)}
                    {c.note && <span className="block text-xs text-amber-600">{t("pos_note")}: {c.note}</span>}
                  </button>
                  <span className="font-bold" dir="ltr">{m(unitPrice(ix, c.config, ctx.happy_hour) * c.qty)}</span>
                </div>
              );
            })}
          </div>
          {order?.loyalty && (
            <div className="flex items-center gap-2 border-t line bg-amber-500/10 px-3 py-1.5 text-[13px]" data-testid="loyalty-strip">
              <Star className="h-4 w-4 shrink-0 text-amber-500" />
              <span className="min-w-0 flex-1 truncate">
                <b>{order.loyalty.customer.name || formatPhone(order.loyalty.customer.phone)}</b>
                {order.loyalty.mode !== "none" && <span className="muted"> · {loyaltyText(order.loyalty.stamps, order.loyalty.points)}</span>}
              </span>
              {order.loyalty.redeemed ? (
                <span className="shrink-0 font-semibold text-pink-600">{t("lo_reward_used")}</span>
              ) : order.loyalty.can_redeem && !cart.length ? (
                <button className="h-9 shrink-0 rounded-sm bg-amber-500 px-3 font-semibold text-black"
                  onClick={() => call("loyalty.redeem", { order_id: order.id }, t("lo_reward_applied"))}>
                  {order.loyalty.mode === "stamps" ? t("lo_use_reward") : t("lo_use_points", { n: order.loyalty.max_points, v: m(pointsValue(order.loyalty.max_points, order.loyalty.point_value_cents)) })}
                </button>
              ) : null}
            </div>
          )}
          <div className="border-t-2 border-[var(--text)] px-3 py-2">
            <div className="muted flex justify-between text-[13px]"><span>HT</span><span dir="ltr">{m(baseShown)}</span></div>
            <div className="muted flex justify-between text-[13px]"><span>{t("rc_vat")}</span><span dir="ltr">{m(taxShown)}</span></div>
            {order?.discount_kind && (
              <div className="flex justify-between text-[13px] text-pink-600">
                <span>{order.loyalty?.redeemed ? t("lo_reward") : t("pos_discount")} {order.discount_kind === "percent" ? formatRate(order.discount_value ?? 0) : ""}</span>
                <span dir="ltr">-{m(estimate?.order_discount ?? 0)}</span>
              </div>
            )}
            {(order?.type ?? draft.type) === "delivery" && (
              <div className="muted flex justify-between text-[13px]"><span>{t("rc_delivery_fee")}</span><span dir="ltr">{m(order ? order.delivery_fee_cents : draft.delivery_fee_cents ?? Number(s.delivery_fee_cents ?? 0))}</span></div>
            )}
            <div className="mt-1 flex items-baseline justify-between">
              <span className="num text-xl font-bold">TOTAL{cart.length > 0 && order ? <span className="muted ms-1 text-xs font-normal">({t("pos_estimate")})</span> : null}</span>
              <span className="text-3xl font-bold" dir="ltr">{m(totalShown)}</span>
            </div>
          </div>
        </section>

        {/* ---------------- products (center) */}
        <section aria-label={t("tab_products")} className="flex min-h-0 flex-col bg-[var(--panel-2)]">
          <div className="flex gap-1 overflow-x-auto border-b line bg-[var(--panel)] p-2">
            {ctx.menu.categories.map((c, i) => (
              <button key={c.id} onClick={() => setCat(c.id)}
                className={`h-10 shrink-0 rounded-sm border px-3 text-[13px] font-semibold ${cat === c.id ? "border-brand bg-brand text-white" : "line bg-[var(--panel)]"}`}>
                <span className="num me-1 opacity-60">{i + 1}</span>{pickName(c.names, t.lang)}
              </button>
            ))}
          </div>
          <div className="grid min-h-0 flex-1 auto-rows-min grid-cols-1 overflow-auto sm:grid-cols-2">
            {products.map((p) => {
              const c = ctx.menu.categories.find((x) => x.id === p.category_id);
              const prices = p.variants.filter((v) => v.active).map((v) => (ctx.happy_hour && v.happy_hour_price_cents !== null ? v.happy_hour_price_cents : v.price_cents));
              const price = prices.length ? Math.min(...prices) : ctx.happy_hour && p.happy_hour_price_cents !== null ? p.happy_hour_price_cents : p.price_cents;
              return (
                <button key={p.id} disabled={!p.available} onClick={() => addProduct(p)}
                  className="grid h-14 grid-cols-[6px_1fr_auto] items-center gap-3 border-b border-e line bg-[var(--panel)] pe-3 text-start hover:bg-[var(--sel)] disabled:bg-[var(--panel-2)] disabled:text-[var(--muted)]">
                  <span className="h-full" style={{ background: c?.color }} />
                  <span className="min-w-0 truncate font-medium">
                    {pickName(p.names, t.lang)}
                    {(p.variants.some((v) => v.active) || p.kind === "combo") && <span className="muted ms-1 text-xs">…</span>}
                  </span>
                  {!p.available ? <span className="text-xs font-bold text-red-600">{t("out_of_stock")}</span> : (
                    <span dir="ltr">{prices.length > 1 ? "≥ " : ""}{m(price)}</span>
                  )}
                </button>
              );
            })}
          </div>
        </section>

        {/* ---------------- right column: status + pay */}
        <section className="flex min-h-0 flex-col gap-2 bg-[var(--panel)] p-3">
          <div className="label">{t("pos_due")}</div>
          <div className="text-4xl font-bold" dir="ltr">{m(order ? (cart.length ? totalShown - order.paid_cents : order.due_cents) : totalShown)}</div>
          {order && order.paid_cents !== 0 && (
            <div className="grid grid-cols-2 gap-px border line bg-[var(--line)]">
              <div className="bg-[var(--panel-2)] p-2"><div className="label">{t("pos_paid")}</div><span className="font-bold" dir="ltr">{m(order.paid_cents)}</span></div>
              <div className="bg-[var(--panel-2)] p-2"><div className="label">{t("rc_due")}</div><span className="font-bold text-amber-600" dir="ltr">{m(order.due_cents)}</span></div>
            </div>
          )}
          <button onClick={() => setDialog({ kind: "orders" })} className="flex h-11 items-center justify-between rounded-sm border line px-3 text-[13px] font-semibold">
            <span className="flex items-center gap-2"><ClipboardList className="h-4 w-4" />{t("pos_orders")}</span>
            {readyCount > 0 && <span className="flex items-center gap-1 rounded-sm bg-emerald-600 px-2 py-0.5 text-xs font-bold text-white"><Bell className="h-3 w-3" />{readyCount}</span>}
          </button>
          {ctx.tables.length > 0 && (
            <button onClick={() => setFloorMode(true)} className="flex h-11 items-center gap-2 rounded-sm border line px-3 text-[13px] font-semibold"><LayoutDashboard className="h-4 w-4" />{t("fl_tables")}</button>
          )}
          <button onClick={newOrder} className="flex h-11 items-center gap-2 rounded-sm border line px-3 text-[13px] font-semibold"><Plus className="h-4 w-4" />{t("pos_new_order")}</button>
          <div className="flex-1" />
          {order && order.status === "closed" ? (
            <Button className="h-16" onClick={() => call("orders.reopen", { order_id: order.id }, t("pos_reopen"))}>{t("pos_reopen")}</Button>
          ) : (
            <>
              <Button className="h-14" disabled={!cart.length || !editable} onClick={send}>
                <ChefHat className="me-2 inline h-5 w-5" />{t("pos_send")}
              </Button>
              <Button variant="primary" className="h-20 text-xl" disabled={!canPay} onClick={startPay}>
                {t("pos_pay").toUpperCase()}
              </Button>
            </>
          )}
        </section>
      </div>

      {/* ---------------- function bar */}
      <div className="bar flex h-12 shrink-0 divide-x divide-white/10">
        <button className={fkey} disabled={!order || !editable || !!cart.length} onClick={() => setDialog({ kind: "discount" })}><Percent />{t("pos_discount")}</button>
        <button className={fkey} disabled={!editable || (!cart.length && !order)} onClick={() => setDialog({ kind: "hold" })}><Pause />{t("pos_hold")}</button>
        <button className={fkey} disabled={!order || !editable || order.paid_cents !== 0 || !!cart.length || order.lines.filter((l) => !l.voided).length < 1} onClick={() => setDialog({ kind: "split" })}><Scissors />{t("pos_split")}</button>
        <button className={fkey} disabled={!order} onClick={() => setDialog({ kind: "receipt", reprint: !!order?.paid_at })}><Receipt />{t("pos_receipt")}</button>
        {order && order.paid_cents > 0 ? (
          <button className={`${fkey} text-red-400`} onClick={() => setDialog({ kind: "refund" })}><X />{t("pos_refund")}</button>
        ) : (
          <button className={`${fkey} text-red-400`} disabled={!order || !editable} onClick={() => setDialog({ kind: "void" })}><Trash2 />{t("pos_void_order")}</button>
        )}
      </div>

      {/* ---------------- dialogs */}
      {configuring && (
        <ItemConfigurator ix={ix} productId={configuring.productId} happy={ctx.happy_hour} initial={configuring.initial}
          onClose={() => setConfiguring(null)}
          onConfirm={(cfg) => { addToCart(cfg, configuring.editKey); setConfiguring(null); }} />
      )}

      {cartSel && (
        <Modal onClose={() => setSheet(null)}>
          <h2 className="mb-3 text-xl font-bold">{pickName(ix.product(cartSel.config.product_id).names, t.lang)}</h2>
          <Field label={t("pos_note")}>
            <TextInput autoFocus value={cartSel.note} placeholder="bien cuit, allergie sésame…" onChange={(e) => setCart((x) => x.map((y) => (y.key === cartSel.key ? { ...y, note: e.target.value } : y)))} />
          </Field>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            {(ix.groupsOf(ix.product(cartSel.config.product_id)).length > 0 || ix.product(cartSel.config.product_id).kind === "combo" || ix.product(cartSel.config.product_id).variants.length > 0) && (
              <Button onClick={() => { setConfiguring({ productId: cartSel.config.product_id, editKey: cartSel.key, initial: cartSel.config }); setSheet(null); }}>{t("edit")}</Button>
            )}
            <Button variant="danger" onClick={() => { setCart((x) => x.filter((y) => y.key !== cartSel.key)); setSheet(null); }}>{t("pos_remove")}</Button>
            <Button variant="primary" onClick={() => setSheet(null)}>{t("close")}</Button>
          </div>
        </Modal>
      )}

      {lineSel && order && (
        <LineSheet line={lineSel} lang={t.lang} onClose={() => setSheet(null)}
          onQty={async (q) => { await call("orders.update_line", { line_id: lineSel.id, qty: q }); }}
          onNote={async (n) => { await call("orders.update_line", { line_id: lineSel.id, note: n }); }}
          onDiscount={() => { setSheet(null); setDialog({ kind: "discount", lineId: lineSel.id }); }}
          onRemove={async () => {
            setSheet(null);
            if (lineSel.kitchen_status === "new") await call("orders.remove_line", { line_id: lineSel.id });
            else setDialog({ kind: "void_line", id: lineSel.id });
          }} />
      )}

      {dialog?.kind === "tables" && (
        <TablePicker
          current={draft.table_id}
          moving={!!order && order.type === "dine_in"}
          onClose={() => setDialog(null)}
          onNone={() => { setDraft({ ...draft, table_id: null }); setDialog(null); }}
          onPick={async (tb) => {
            if (order && order.type === "dine_in") {
              // A saved dine-in order: move it to the tapped table.
              if (tb.id === order.table_id) return setDialog(null);
              try {
                await rpc("orders.move_table", { order_id: order.id, table_id: tb.id });
                await load(order.id);
              } catch (e) {
                fail(e);
              }
            } else if (!order && !cart.length && tb.orders.length) {
              // Nothing in progress: open the order already on that table.
              await load(tb.orders[tb.orders.length - 1].id);
            } else {
              setDraft({ ...draft, type: "dine_in", table_id: tb.id });
              setDialog(null);
            }
          }}
        />
      )}

      {dialog?.kind === "hold" && (
        <HoldDialog onCancel={() => setDialog(null)} initial={draft.customer_name || ctx.tables.find((x) => x.id === draft.table_id)?.name || ""}
          onConfirm={async (label) => {
            try {
              await submit({ hold: true, hold_label: label });
              toast.ok(t("pos_hold"));
              newOrder();
            } catch (e) {
              fail(e);
            }
          }} />
      )}

      {dialog?.kind === "discount" && order && (
        <DiscountDialog
          forLine={dialog.lineId !== undefined}
          current={dialog.lineId !== undefined ? order.lines.find((l) => l.id === dialog.lineId)?.discount_kind ?? null : order.discount_kind}
          onCancel={() => setDialog(null)}
          onApply={async (discount) => {
            const r = await call("orders.discount", dialog.lineId !== undefined ? { line_id: dialog.lineId, discount } : { order_id: order.id, discount });
            if (r) setDialog(null);
          }} />
      )}

      {dialog?.kind === "void" && order && (
        <ReasonDialog danger title={t("pos_void_order")} presets={["Erreur de saisie", "Client parti", "Commande en double"]} onCancel={() => setDialog(null)}
          onConfirm={async (reason) => { const r = await call("orders.void", { order_id: order.id, reason }, t("status_voided")); if (r) newOrder(); }} />
      )}
      {dialog?.kind === "void_line" && (
        <ReasonDialog danger title={t("pos_void_line")} presets={["Erreur de saisie", "Client a changé d'avis", "Produit raté"]} onCancel={() => setDialog(null)}
          onConfirm={async (reason) => { await call("orders.remove_line", { line_id: dialog.id, reason }); setDialog(null); }} />
      )}
      {dialog?.kind === "refund" && order && (
        <ReasonDialog danger title={`${t("pos_refund")} ${m(order.paid_cents)}`} presets={["Client mécontent", "Erreur de commande", "Produit indisponible"]} onCancel={() => setDialog(null)}
          onConfirm={async (reason) => { const r = await call("orders.refund", { order_id: order.id, reason }, t("pay_refunded")); if (r) setDialog(null); }} />
      )}

      {dialog?.kind === "split" && order && (
        <SplitDialog order={order} lang={t.lang} onCancel={() => setDialog(null)}
          onConfirm={async (moves) => {
            try {
              const r = await rpc<{ original: Order; new: Order }>("orders.split", { order_id: order.id, moves });
              setOrder(r.original);
              setDialog(null);
              toast.ok(`#${orderLabel(r.new)} ✓`);
            } catch (e) {
              fail(e);
            }
          }} />
      )}

      {dialog?.kind === "pay" && order && (
        <PaymentDialog due={order.due_cents} onCancel={() => setDialog(null)}
          onPay={async ({ payments }) => {
            const r = await rpc<{ result: { change_cents: number }; order: Order }>("orders.pay", { order_id: order.id, payments, register });
            setOrder(r.order);
            setDialog({ kind: "paid", change: r.result.change_cents });
            const paidCash = payments.some((x) => x.method === "cash");
            if (ctx.receipt_printer && s.auto_print_receipt !== false) {
              printReceipt(r.order, s, ctx.receipt_printer, { lang: receiptLang, register, kick: paidCash && s.drawer_on_cash !== false }).catch(fail);
            } else if (paidCash && s.drawer_on_cash !== false && ctx.receipt_printer?.drawer) {
              rpc("drawer.open", { reason: "cash payment", register }).catch(fail);
            }
          }} />
      )}

      {dialog?.kind === "paid" && order && (
        <Modal onClose={newOrder} wide>
          <div className="grid gap-5 md:grid-cols-2">
            <div className="flex flex-col items-center justify-center gap-3 text-center">
              <div className="text-5xl">✅</div>
              <div className="text-xl font-bold">{t("pos_payment_done")} · #{orderLabel(order)}</div>
              {dialog.change > 0 && (
                <div className="rounded-2xl bg-emerald-600/15 px-8 py-4">
                  <div className="muted">{t("pos_change")}</div>
                  <div className="text-5xl font-bold text-emerald-500" dir="ltr">{m(dialog.change)}</div>
                </div>
              )}
              <Button variant="primary" className="w-full text-lg" onClick={newOrder}>{t("pos_new_order")}</Button>
              {me.permissions.includes("drawer.open") && ctx.receipt_printer?.drawer && <Button className="w-full" onClick={() => rpc("drawer.open", { reason: "manual", register }).catch(fail)}>{t("pos_open_drawer")}</Button>}
              {ctx.receipt_printer && <Button className="w-full" onClick={() => printReceipt(order, s, ctx.receipt_printer!, { lang: receiptLang, register, reprint: true }).then(() => toast.ok(t("pr_sent")), fail)}>{t("pos_reprint")}</Button>}
            </div>
            <div className="max-h-[70vh] overflow-auto">
              <ReceiptView width={getPaper()} lines={buildReceipt(order, s, { lang: receiptLang, width: getPaper() })} />
            </div>
          </div>
        </Modal>
      )}

      {dialog?.kind === "receipt" && order && (
        <Modal onClose={() => setDialog(null)}>
          <ReceiptView width={getPaper()} lines={buildReceipt(order, s, { lang: receiptLang, width: getPaper(), reprint: dialog.reprint })} />
          {!ctx.receipt_printer && <p className="muted mt-3 text-center text-xs">{t("pr_no_receipt_printer")}</p>}
          <div className="mt-3 flex flex-wrap justify-end gap-2">
            {order.lines.some((l) => l.kitchen_status !== "new" && !l.voided) && (
              <Button onClick={() => rpc("print.kitchen_reprint", { id: order.id }).then(() => toast.ok(t("pr_sent")), fail)}>{t("pr_print_kitchen")}</Button>
            )}
            {ctx.receipt_printer ? (
              <Button variant="primary" onClick={() => printReceipt(order, s, ctx.receipt_printer!, { lang: receiptLang, register, reprint: dialog.reprint }).then(() => { toast.ok(t("pr_sent")); setDialog(null); }, fail)}>
                {dialog.reprint ? t("pos_reprint") : t("pr_print")}
              </Button>
            ) : (
              <Button onClick={() => { rpc("orders.receipt_printed", { order_id: order.id, reprint: dialog.reprint }).catch(fail); setDialog(null); }}>{t("close")}</Button>
            )}
          </div>
        </Modal>
      )}

      {dialog?.kind === "orders" && <OrdersDialog onClose={() => setDialog(null)} onOpen={load} onStatus={async (id, st) => { await rpc("orders.set_status", { order_id: id, status: st }).catch(fail); if (order?.id === id) load(id); }} />}

      {dialog?.kind === "open_till" && (
        <OpenTillDialog onCancel={() => setDialog(null)} onOpen={async (float_cents) => {
          try {
            await rpc("cash.open", { register, float_cents });
            await loadCtx();
            setDialog(null);
          } catch (e) {
            fail(e);
          }
        }} />
      )}

      {approval.node}
      {toast.node}
    </div>
  );
}

function LineSheet({ line, lang, onClose, onQty, onNote, onDiscount, onRemove }: {
  line: OrderLine; lang: Lang; onClose: () => void; onQty: (q: number) => Promise<void>; onNote: (n: string) => Promise<void>; onDiscount: () => void; onRemove: () => Promise<void>;
}) {
  const t = useT();
  const [note, setNote] = useState(line.note);
  const unsent = line.kitchen_status === "new";
  return (
    <Modal onClose={onClose}>
      <h2 className="text-xl font-bold">{line.qty}× {pickName(line.names, lang)}</h2>
      <p className="muted mb-3 text-sm">{(line.details[lang] ?? line.details.fr).join(" · ")}</p>
      {unsent && !line.voided && (
        <div className="mb-3 flex items-center gap-3" dir="ltr">
          <Button onClick={() => line.qty > 1 && onQty(line.qty - 1)}><Minus /></Button>
          <span className="w-10 text-center text-2xl font-bold">{line.qty}</span>
          <Button onClick={() => onQty(line.qty + 1)}><Plus /></Button>
        </div>
      )}
      {!line.voided && (
        <>
          <Field label={t("pos_note")}>
            <TextInput value={note} onChange={(e) => setNote(e.target.value)} onBlur={() => note !== line.note && onNote(note)} />
          </Field>
          <div className="mt-4 flex flex-wrap justify-end gap-2">
            <Button onClick={onDiscount}><Percent className="inline h-4 w-4" /> {t("pos_discount_line")}</Button>
            <Button variant="danger" onClick={onRemove}>{unsent ? t("pos_remove") : t("pos_void_line")}</Button>
            <Button variant="primary" onClick={onClose}>{t("close")}</Button>
          </div>
        </>
      )}
    </Modal>
  );
}

function HoldDialog({ initial, onCancel, onConfirm }: { initial: string; onCancel: () => void; onConfirm: (label: string) => void }) {
  const t = useT();
  const [label, setLabel] = useState(initial);
  return (
    <Modal onClose={onCancel}>
      <Field label={t("pos_hold_label")}>
        <TextInput autoFocus value={label} onChange={(e) => setLabel(e.target.value)} />
      </Field>
      <div className="mt-4 flex justify-end gap-3">
        <Button onClick={onCancel}>{t("cancel")}</Button>
        <Button variant="primary" onClick={() => onConfirm(label)}>{t("pos_hold")}</Button>
      </div>
    </Modal>
  );
}

function DiscountDialog({ forLine, current, onCancel, onApply }: {
  forLine: boolean; current: string | null; onCancel: () => void; onApply: (d: { kind: string; value: number; reason: string } | null) => void;
}) {
  const t = useT();
  const [kind, setKind] = useState<"percent" | "fixed" | "comp">("percent");
  const [pct, setPct] = useState(10);
  const [cents, setCents] = useState<number | null>(1000);
  const [reason, setReason] = useState("");
  const kinds: ("percent" | "fixed" | "comp")[] = forLine ? ["percent", "fixed", "comp"] : ["percent", "fixed"];
  const value = kind === "percent" ? Math.round(pct * 100) : kind === "fixed" ? cents ?? 0 : 0;
  const ok = kind === "comp" ? reason.trim().length > 0 : value > 0 && (kind !== "percent" || value <= 10000);
  return (
    <Modal onClose={onCancel}>
      <h2 className="mb-3 text-xl font-bold">{forLine ? t("pos_discount_line") : t("pos_discount_order")}</h2>
      <div className="mb-3 grid grid-cols-3 gap-2">
        {kinds.map((k) => (
          <button key={k} onClick={() => setKind(k)} className={`min-h-12 rounded-xl border font-semibold ${kind === k ? "border-brand bg-brand/20" : "line panel-2"}`}>
            {t(k === "percent" ? "pos_percent" : k === "fixed" ? "pos_fixed" : "pos_comp")}
          </button>
        ))}
      </div>
      {kind === "percent" && (
        <div className="mb-3 flex flex-wrap gap-2">
          {[5, 10, 15, 20, 25, 50].map((v) => (
            <button key={v} onClick={() => setPct(v)} className={`min-h-12 min-w-16 rounded-xl border font-bold ${pct === v ? "border-brand bg-brand/20" : "line panel-2"}`}>{v} %</button>
          ))}
          <TextInput className="max-w-28" type="number" min={1} max={100} value={pct} onChange={(e) => setPct(Number(e.target.value))} />
        </div>
      )}
      {kind === "fixed" && <div className="mb-3"><MoneyField cents={cents} onChange={setCents} /></div>}
      <TextInput placeholder={`${t("pos_reason")}${kind === "comp" ? ` (${t("pos_required")})` : ""}`} value={reason} onChange={(e) => setReason(e.target.value)} />
      <div className="mt-4 flex flex-wrap justify-end gap-2">
        {current && <Button onClick={() => onApply(null)}>{t("pos_remove_discount")}</Button>}
        <Button onClick={onCancel}>{t("cancel")}</Button>
        <Button variant="primary" disabled={!ok} onClick={() => onApply({ kind, value, reason })}>{t("pos_apply")}</Button>
      </div>
    </Modal>
  );
}

function SplitDialog({ order, lang, onCancel, onConfirm }: { order: Order; lang: Lang; onCancel: () => void; onConfirm: (moves: { line_id: number; qty: number }[]) => void }) {
  const t = useT();
  const [qty, setQty] = useState<Record<number, number>>({});
  const lines = order.lines.filter((l) => !l.voided);
  const moves = Object.entries(qty).filter(([, q]) => q > 0).map(([id, q]) => ({ line_id: Number(id), qty: q }));
  return (
    <Modal onClose={onCancel}>
      <h2 className="text-xl font-bold">{t("pos_split")}</h2>
      <p className="muted mb-3 text-sm">{t("pos_split_hint")}</p>
      {lines.map((l) => (
        <div key={l.id} className="flex items-center gap-2 border-b line py-2">
          <span className="flex-1">{l.qty}× {pickName(l.names, lang)}</span>
          <div className="flex items-center gap-1" dir="ltr">
            <button aria-label="-" className="h-11 w-11 rounded-lg border line" onClick={() => setQty({ ...qty, [l.id]: Math.max(0, (qty[l.id] ?? 0) - 1) })}><Minus className="mx-auto h-4 w-4" /></button>
            <span className="w-6 text-center font-bold">{qty[l.id] ?? 0}</span>
            <button aria-label="+" className="h-11 w-11 rounded-lg border line" onClick={() => setQty({ ...qty, [l.id]: Math.min(l.qty, (qty[l.id] ?? 0) + 1) })}><Plus className="mx-auto h-4 w-4" /></button>
          </div>
        </div>
      ))}
      <div className="mt-4 flex justify-end gap-3">
        <Button onClick={onCancel}>{t("cancel")}</Button>
        <Button variant="primary" disabled={!moves.length} onClick={() => onConfirm(moves)}>{t("pos_split")}</Button>
      </div>
    </Modal>
  );
}

interface OrderRow { id: number; number: number; split_index: number; type: OrderType; table_name: string | null; customer_name: string; status: string; payment_status: string; held: boolean; hold_label: string; total_cents: number; paid_cents: number; created_at: string }

function OrdersDialog({ onClose, onOpen, onStatus }: { onClose: () => void; onOpen: (id: number) => void; onStatus: (id: number, s: string) => Promise<void> }) {
  const t = useT();
  const [scope, setScope] = useState<"active" | "held" | "today">("active");
  const [rows, setRows] = useState<OrderRow[]>([]);
  const load = useCallback(() => rpc<OrderRow[]>("orders.list", { scope }).then(setRows, () => {}), [scope]);
  useEffect(() => void load(), [load]);
  const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return (
    <Modal onClose={onClose} wide>
      <div className="mb-3 flex gap-2">
        {(["active", "held", "today"] as const).map((sc) => (
          <button key={sc} onClick={() => setScope(sc)} className={`min-h-12 rounded-xl px-4 font-semibold ${scope === sc ? "bg-brand text-white" : "panel-2 border line"}`}>
            {sc === "active" ? t("pos_orders") : sc === "held" ? t("pos_held") : t("all")}
          </button>
        ))}
      </div>
      <div className="flex flex-col divide-y line">
        {rows.map((r) => (
          <div key={r.id} className="flex flex-wrap items-center gap-2 py-2">
            <button className="flex min-w-0 flex-1 items-center gap-3 text-start" onClick={() => onOpen(r.id)}>
              <span className="w-14 text-lg font-bold">#{orderLabel(r)}</span>
              <span className="min-w-0">
                <span className="block truncate font-medium">
                  {t(`order_type_${r.type}` as Key)}{r.table_name ? ` · ${r.table_name}` : ""}{r.customer_name ? ` · ${r.customer_name}` : ""}{r.held && r.hold_label ? ` · ⏸ ${r.hold_label}` : ""}
                </span>
                <span className="muted block text-xs">{time(r.created_at)} · {t(`status_${r.status}` as Key)} · {t(`pay_${r.payment_status}` as Key)}</span>
              </span>
            </button>
            <span className="font-semibold" dir="ltr">{formatMoney(r.total_cents, t.lang)}</span>
            {["sent", "preparing"].includes(r.status) && <Button onClick={() => onStatus(r.id, "ready").then(load)}>{t("pos_mark_ready")}</Button>}
            {r.status === "ready" && <Button variant="primary" onClick={() => onStatus(r.id, "served").then(load)}>{t("pos_mark_served")}</Button>}
          </div>
        ))}
        {!rows.length && <p className="muted py-6 text-center">—</p>}
      </div>
    </Modal>
  );
}

function OpenTillDialog({ onCancel, onOpen }: { onCancel: () => void; onOpen: (float: number) => void }) {
  const t = useT();
  const [float, setFloat] = useState<number | null>(50000);
  return (
    <Modal onClose={onCancel}>
      <h2 className="mb-3 text-xl font-bold">{t("pos_open_till")}</h2>
      <Field label={t("cash_float")}><MoneyField cents={float} onChange={setFloat} /></Field>
      <div className="mt-4 flex justify-end gap-3">
        <Button onClick={onCancel}>{t("cancel")}</Button>
        <Button variant="primary" onClick={() => onOpen(float ?? 0)}>{t("cash_open")}</Button>
      </div>
    </Modal>
  );
}

/** POS table picker: the live floor plan. Free tables take the new order; busy ones open theirs. */
function TablePicker({ current, moving, onClose, onNone, onPick }: {
  current: number | null; moving: boolean; onClose: () => void; onNone: () => void; onPick: (t: PlanTable) => void;
}) {
  const t = useT();
  const { plan } = usePlan();
  const [floorId, setFloorId] = useState<number | null>(null);
  const floors = plan?.floors.filter((f) => f.active) ?? [];
  const shown = floorId ?? plan?.tables.find((x) => x.id === current)?.floor_id ?? floors[0]?.id ?? null;
  return (
    <Modal wide onClose={onClose}>
      <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
        <h2 className="text-xl font-bold">{moving ? t("fl_move_to") : t("pos_select_table")}</h2>
        {!moving && <button className="h-10 rounded-md border line panel-2 px-4" onClick={onNone}>{t("pos_no_table")}</button>}
      </div>
      {!plan ? (
        <p className="muted">…</p>
      ) : (
        <>
          <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
            <FloorTabs floors={floors} value={shown} onChange={setFloorId} />
            <StateLegend />
          </div>
          <FloorBoard plan={plan} floorId={shown} selectedId={current} onTap={onPick} />
        </>
      )}
    </Modal>
  );
}

/** Start of service: the floor plan with live table status, plus takeaway / delivery shortcuts. */
function FloorHome({ onTable, onTakeaway, onDelivery, canEdit }: { onTable: (t: PlanTable) => void; onTakeaway: () => void; onDelivery: () => void; canEdit: boolean }) {
  const t = useT();
  const [editing, setEditing] = useState(false);
  const { plan } = usePlan();
  const [floorId, setFloorId] = useState<number | null>(null);
  const floors = plan?.floors.filter((f) => f.active) ?? [];
  const shown = floorId ?? floors[0]?.id ?? null;
  if (editing) return <div className="min-h-0 flex-1 overflow-auto"><FloorScreen onDone={() => setEditing(false)} /></div>;
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-auto p-3">
      <div className="flex flex-wrap items-center gap-2">
        <FloorTabs floors={floors} value={shown} onChange={setFloorId} />
        <div className="flex-1" />
        <StateLegend />
        {canEdit && <button onClick={() => setEditing(true)} className="h-11 rounded-sm border line panel-2 px-4 font-semibold">{t("fl_edit_plan")}</button>}
        <button onClick={onTakeaway} className="h-11 rounded-sm bg-brand px-5 font-semibold text-white">{t("order_type_takeaway")}</button>
        <button onClick={onDelivery} className="h-11 rounded-sm border line panel-2 px-5 font-semibold">{t("order_type_delivery")}</button>
      </div>
      {plan ? (
        <div className="mx-auto w-full max-w-[min(100%,calc((100vh-180px)*1.5))]">
          <FloorBoard plan={plan} floorId={shown} onTap={onTable} />
        </div>
      ) : (
        <p className="muted">…</p>
      )}
    </div>
  );
}
