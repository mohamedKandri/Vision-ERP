// Pure money + VAT logic. All amounts are integer cents (centimes of MAD).
// Tax rates are basis points: 1000 = 10.00 %.
// Rounding is half away from zero, matching what a customer expects on a receipt.

export type Cents = number;

export interface Tax {
  id: number;
  rate_bp: number;
  /** true = product price already includes this tax (TTC), like Odoo's "Included in price". */
  price_include: boolean;
}

export type Discount = { kind: "percent"; bp: number } | { kind: "fixed"; cents: Cents } | null;

function assertInt(n: number, what: string) {
  if (!Number.isSafeInteger(n)) throw new Error(`${what} must be a safe integer, got ${n}`);
}

/** Integer division rounded half away from zero. */
export function divRound(numerator: number, denominator: number): number {
  assertInt(numerator, "numerator");
  assertInt(denominator, "denominator");
  if (denominator === 0) throw new Error("division by zero");
  const sign = Math.sign(numerator) * Math.sign(denominator);
  const n = Math.abs(numerator);
  const d = Math.abs(denominator);
  const q = Math.floor(n / d);
  const r = n - q * d;
  return sign * (2 * r >= d ? q + 1 : q);
}

/** Percentage (bp) of an amount, rounded. */
export function percentOf(amount: Cents, bp: number): Cents {
  return divRound(amount * bp, 10000);
}

/** Split an amount that includes tax into base + tax. */
export function splitIncluded(total: Cents, rateBp: number): { base: Cents; tax: Cents } {
  const base = divRound(total * 10000, 10000 + rateBp);
  return { base, tax: total - base };
}

/** Amount the discount takes off `gross`; never more than gross, never negative. */
export function discountAmount(gross: Cents, d: Discount): Cents {
  if (!d) return 0;
  if (d.kind === "percent") {
    if (d.bp < 0 || d.bp > 10000) throw new Error("discount percent out of range");
    return percentOf(gross, d.bp);
  }
  if (d.cents < 0) throw new Error("negative discount");
  return Math.min(d.cents, gross);
}

export interface LineInput {
  unit_cents: Cents; // price as entered on the product (TTC or HT depending on tax)
  qty: number; // integer quantity
  tax: Tax;
  discount?: Discount;
}

export interface LineTotals {
  gross: Cents; // unit × qty, in the product's price basis
  discount: Cents;
  base: Cents; // HT
  tax: Cents;
  total: Cents; // TTC, what the customer pays
}

export function computeLine(l: LineInput): LineTotals {
  assertInt(l.unit_cents, "unit price");
  assertInt(l.qty, "qty");
  if (l.qty < 0) throw new Error("qty must be ≥ 0 (use a refund for returns)");
  const gross = l.unit_cents * l.qty;
  const discount = discountAmount(gross, l.discount ?? null);
  const net = gross - discount;
  if (l.tax.price_include) {
    const { base, tax } = splitIncluded(net, l.tax.rate_bp);
    return { gross, discount, base, tax, total: net };
  }
  const tax = percentOf(net, l.tax.rate_bp);
  return { gross, discount, base: net, tax, total: net + tax };
}

/**
 * Split `amount` across `weights` proportionally so the parts always sum to `amount`
 * (largest-remainder method). Used to spread an order discount over lines.
 */
export function allocate(amount: Cents, weights: number[]): Cents[] {
  assertInt(amount, "amount");
  const sum = weights.reduce((a, b) => a + b, 0);
  if (weights.length === 0) return [];
  if (sum === 0) return weights.map((_, i) => (i === 0 ? amount : 0));
  const sign = Math.sign(amount);
  const abs = Math.abs(amount);
  const raw = weights.map((w) => (abs * w) / sum);
  const parts = raw.map(Math.floor);
  let rest = abs - parts.reduce((a, b) => a + b, 0);
  const order = raw
    .map((r, i) => ({ i, frac: r - Math.floor(r) }))
    .sort((a, b) => b.frac - a.frac || a.i - b.i);
  for (let k = 0; rest > 0; k++, rest--) parts[order[k % order.length].i]++;
  return parts.map((p) => p * sign);
}

export interface TaxBucket {
  tax_id: number;
  rate_bp: number;
  base: Cents;
  tax: Cents;
  total: Cents;
}

export interface OrderTotals {
  lines: LineTotals[];
  subtotal: Cents; // sum of line totals before order discount (TTC)
  order_discount: Cents; // TTC amount taken off by the order-level discount
  base: Cents;
  tax: Cents;
  total: Cents;
  vat: TaxBucket[];
}

export type TaxRounding = "per_line" | "global";

/**
 * Totals for a whole order.
 * - Line discounts apply first, then the order discount is spread over lines by their totals.
 * - per_line: VAT is rounded on each line, then summed.
 * - global: VAT is computed once per tax on the summed amounts (fewer rounding cents).
 * Total is always base + tax, and always equals the sum of what each line charges.
 */
export function computeOrder(
  lines: LineInput[],
  opts: { rounding?: TaxRounding; orderDiscount?: Discount; extraFee?: { cents: Cents; tax: Tax } } = {},
): OrderTotals {
  const rounding = opts.rounding ?? "per_line";
  const first = lines.map(computeLine);
  const subtotal = first.reduce((a, l) => a + l.total, 0);
  const orderDiscount = discountAmount(subtotal, opts.orderDiscount ?? null);
  const shares = allocate(orderDiscount, first.map((l) => l.total));

  // Net amount of each line in its own price basis after the order discount.
  type Net = { tax: Tax; net: Cents };
  const nets: Net[] = lines.map((l, i) => {
    const lt = first[i];
    if (l.tax.price_include) return { tax: l.tax, net: lt.total - shares[i] };
    // Share is TTC; convert to HT for an excluded tax.
    const shareHt = divRound(shares[i] * 10000, 10000 + l.tax.rate_bp);
    return { tax: l.tax, net: lt.base - shareHt };
  });
  if (opts.extraFee && opts.extraFee.cents > 0) nets.push({ tax: opts.extraFee.tax, net: opts.extraFee.cents });

  const finalLines: LineTotals[] = lines.map((_, i) => {
    const n = nets[i];
    const lt = first[i];
    const split = n.tax.price_include
      ? { ...splitIncluded(n.net, n.tax.rate_bp), total: n.net }
      : { base: n.net, tax: percentOf(n.net, n.tax.rate_bp), total: n.net + percentOf(n.net, n.tax.rate_bp) };
    return { gross: lt.gross, discount: lt.discount + (lt.total - split.total), ...split };
  });

  const buckets = new Map<string, TaxBucket & { include: boolean; nets: Cents; lineBase: Cents; lineTax: Cents }>();
  nets.forEach((n, i) => {
    const key = `${n.tax.id}`;
    const b =
      buckets.get(key) ??
      { tax_id: n.tax.id, rate_bp: n.tax.rate_bp, base: 0, tax: 0, total: 0, include: n.tax.price_include, nets: 0, lineBase: 0, lineTax: 0 };
    const lt =
      i < finalLines.length
        ? finalLines[i]
        : computeLine({ unit_cents: n.net, qty: 1, tax: n.tax });
    b.nets += n.net;
    b.lineBase += lt.base;
    b.lineTax += lt.tax;
    buckets.set(key, b);
  });

  const vat: TaxBucket[] = [...buckets.values()]
    .map((b) => {
      if (rounding === "per_line") return { tax_id: b.tax_id, rate_bp: b.rate_bp, base: b.lineBase, tax: b.lineTax, total: b.lineBase + b.lineTax };
      if (b.include) {
        const s = splitIncluded(b.nets, b.rate_bp);
        return { tax_id: b.tax_id, rate_bp: b.rate_bp, base: s.base, tax: s.tax, total: b.nets };
      }
      const t = percentOf(b.nets, b.rate_bp);
      return { tax_id: b.tax_id, rate_bp: b.rate_bp, base: b.nets, tax: t, total: b.nets + t };
    })
    .sort((a, b) => a.rate_bp - b.rate_bp || a.tax_id - b.tax_id);

  const base = vat.reduce((a, b) => a + b.base, 0);
  const tax = vat.reduce((a, b) => a + b.tax, 0);
  return { lines: finalLines, subtotal, order_discount: orderDiscount, base, tax, total: base + tax, vat };
}

/** Change to give back for a cash payment. Throws if not enough cash was handed over. */
export function changeDue(due: Cents, tendered: Cents): Cents {
  assertInt(due, "due");
  assertInt(tendered, "tendered");
  if (tendered < due) throw new Error("insufficient cash");
  return tendered - due;
}

/** Parse what a cashier types ("45", "45,5", "45.50", "1 200,00") into cents. Returns null if invalid. */
export function parseMoney(input: string): Cents | null {
  const s = input.trim().replace(/[\s  ]/g, "").replace(/(DH|MAD|د\.م\.)$/i, "");
  if (!/^-?\d+([.,]\d{0,2})?$/.test(s)) return null;
  const [int, frac = ""] = s.replace(",", ".").split(".");
  const neg = int.startsWith("-");
  const cents = Math.abs(parseInt(int, 10)) * 100 + parseInt((frac + "00").slice(0, 2), 10);
  return neg ? -cents : cents;
}

const LOCALES: Record<string, string> = { fr: "fr-MA", ar: "ar-MA", en: "en-MA" };

/** "45,00 MAD" style display. Uses Latin digits in every language (easier at the till). */
export function formatMoney(cents: Cents, lang: string = "fr"): string {
  const nf = new Intl.NumberFormat(`${LOCALES[lang] ?? "fr-MA"}-u-nu-latn`, {
    minimumFractionDigits: 2,
    maximumFractionDigits: 2,
  });
  return `${nf.format(cents / 100)} ${lang === "ar" ? "د.م." : "DH"}`;
}

/** "10 %" from basis points. */
export function formatRate(bp: number): string {
  const v = bp / 100;
  return `${Number.isInteger(v) ? v : v.toFixed(2).replace(/0$/, "")} %`;
}
