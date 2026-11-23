import { Calendar, ChevronDown, Download, FileText, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Button, Card, Field, TextInput, useToast } from "../components/ui";
import { errorText, translate, useT, type Key } from "../i18n";
import { useLive } from "../lib/live";
import { pickName } from "../lib/menu";
import { formatMoney } from "../lib/money";
import {
  addDays, cellText, dayKey, dayRange, exportName, formatPercent, isNum, parseDay, presetDays, PRESETS, reportTables, startOfDay, toBase64, toCsv,
  type Preset, type RangeIn, type Report, type Table,
} from "../lib/report";
import { renderReportPdf } from "../lib/reportPdf";
import { rpc, type Lang } from "../lib/rpc";

const tr = (lang: Lang) => (k: string, v?: Record<string, string | number>) => translate(lang, k as Key, v);
const loc = (lang: Lang) => `${lang === "ar" ? "ar-MA" : lang === "en" ? "en-GB" : "fr-FR"}-u-nu-latn`;
const fmtDateTime = (lang: Lang) => {
  const f = new Intl.DateTimeFormat(loc(lang), { dateStyle: "short", timeStyle: "short" });
  return (iso: string) => f.format(new Date(iso));
};
const fmtDay = (lang: Lang, d: Date, style: "medium" | "full" = "medium") => new Intl.DateTimeFormat(loc(lang), { dateStyle: style }).format(d);

/** The period just before [first, last], same length. When the period ends today it stops at
 *  the same time of day, so a day in progress is compared fairly. */
function previousRange(first: Date, last: Date, now: Date): RangeIn {
  const len = Math.round((startOfDay(last).getTime() - startOfDay(first).getTime()) / 86_400_000) + 1;
  const pFirst = addDays(first, -len);
  if (dayKey(last) === dayKey(now)) {
    const end = new Date(now);
    end.setDate(end.getDate() - len);
    const from = startOfDay(pFirst);
    return { from: from.toISOString(), to: end.toISOString(), tz_offset_min: -from.getTimezoneOffset() };
  }
  return dayRange(pFirst, addDays(last, -len));
}

/** "42,1 %" stays in that order in Arabic; amounts ("616,00 د.م.") are left as they are. */
const pctLtr = (s: string) => (s.includes("%") ? <span dir="ltr">{s}</span> : s);

type Section = "overview" | string;
const GROUPS: { label: Key; ids: string[] }[] = [
  { label: "rp_grp_sales", ids: ["summary", "by_day", "by_hour", "by_product", "by_category", "by_type", "by_payment", "by_staff"] },
  { label: "rp_grp_tax", ids: ["vat"] },
  { label: "rp_grp_costs", ids: ["food_cost", "discounts", "voids", "waste", "waste_items"] },
  { label: "rp_grp_stock", ids: ["stock"] },
];

export function ReportsScreen() {
  const t = useT();
  const toast = useToast();
  const [preset, setPreset] = useState<Preset | "custom">("today");
  const [days, setDays] = useState<[string, string]>(() => presetDays("today", new Date()).map(dayKey) as [string, string]);
  const [rep, setRep] = useState<Report | null>(null);
  const [prev, setPrev] = useState<Report | null>(null);
  const [busy, setBusy] = useState(false);
  const [shop, setShop] = useState("");
  const [cats, setCats] = useState<Record<number, string>>({});
  const [section, setSection] = useState<Section>("overview");
  const [picking, setPicking] = useState(false);

  useEffect(() => {
    rpc<Record<string, unknown>>("settings.get").then((s) => setShop(String(s.shop_name ?? "")), () => {});
    rpc<{ categories: { id: number; names: Record<string, string> }[] }>("menu.get").then(
      (m) => setCats(Object.fromEntries(m.categories.map((c) => [c.id, pickName(c.names, t.lang)]))),
      () => {},
    );
  }, [t.lang]);

  const first = parseDay(days[0]);
  const last = parseDay(days[1]);
  const valid = !!first && !!last && first <= last;
  const load = useCallback(() => {
    if (!first || !last || first > last) return;
    setBusy(true);
    Promise.all([rpc<Report>("reports.summary", dayRange(first, last)), rpc<Report>("reports.summary", previousRange(first, last, new Date()))]).then(
      ([a, b]) => {
        setRep(a);
        setPrev(b);
        setBusy(false);
      },
      (e) => {
        setBusy(false);
        toast.error(errorText(t.lang, e));
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [days[0], days[1], t.lang]);
  useEffect(() => void load(), [load]);
  // A period that includes today follows new sales live.
  const live = !!last && dayKey(last) === dayKey(new Date());
  useLive(["orders", "cash"], () => live && load(), 30000);

  const tables = useMemo(() => (rep ? reportTables(rep, tr(t.lang), t.lang, fmtDateTime(t.lang), (id) => cats[id] ?? "") : []), [rep, t.lang, cats]);
  const period = first && last ? (days[0] === days[1] ? fmtDay(t.lang, first, "full") : t("rp_from_to", { a: fmtDay(t.lang, first), b: fmtDay(t.lang, last) })) : "";
  const prevLabel = (() => {
    if (!first || !last) return "";
    if (days[0] === days[1] && live) return t("rp_compared_yday");
    const r = previousRange(first, last, new Date());
    const a = new Date(r.from);
    const b = addDays(new Date(r.to), live ? 0 : -1);
    return t("rp_compared_prev", { p: dayKey(a) === dayKey(b) ? fmtDay(t.lang, a) : t("rp_from_to", { a: fmtDay(t.lang, a), b: fmtDay(t.lang, b) }) });
  })();
  const presetLabel = preset === "custom" ? t("rp_custom") : t(`rp_preset_${preset}` as Key);

  const save = async (ext: "csv" | "pdf") => {
    if (!rep) return;
    try {
      const head = [shop, `${t("rp_title")} · ${period}`, t("rp_generated", { d: fmtDateTime(t.lang)(new Date().toISOString()) })];
      const bytes = ext === "csv"
        ? new TextEncoder().encode(toCsv(head, tables, t.lang))
        : renderReportPdf({ shop, title: t("rp_title"), lines: [period, head[2]], footer: (p, n) => t("rp_page", { p, n }) }, tables, t.lang);
      const r = await rpc<{ path: string }>("device.save_export", { name: exportName(t("rp_file_prefix"), rep.range, ext), data: toBase64(bytes) });
      toast.ok(t("rp_saved", { p: r.path }));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  const shown = section === "overview" ? null : tables.find((x) => x.id === section) ?? null;
  const cmd = "flex min-h-10 items-center gap-1.5 rounded-md px-2.5 hov disabled:pointer-events-none disabled:opacity-35 [&>svg]:h-4 [&>svg]:w-4 [&>svg]:text-brand";

  return (
    <div className="flex min-h-full flex-col">
      {toast.node}
      <div role="toolbar" aria-label={t("rp_period")} className="relative flex flex-wrap items-center gap-1 border-b line bg-[var(--panel)] px-4 py-1.5 text-[13px]">
        <button onClick={() => setPicking((x) => !x)} aria-expanded={picking} data-testid="rp-period"
          className="hov flex min-h-10 items-center gap-2 rounded-md border line-strong px-3 font-semibold">
          <Calendar className="h-4 w-4 text-brand" />{presetLabel}<ChevronDown className="h-4 w-4" />
        </button>
        <span className="muted px-2">{period}{prevLabel ? ` · ${prevLabel}` : ""}</span>
        <span className="flex-1" />
        <button className={cmd} onClick={load} disabled={!valid || busy}><RefreshCw className={busy ? "animate-spin" : ""} />{t("rp_refresh")}</button>
        <button className={cmd} onClick={() => save("csv")} disabled={!rep}><Download />{t("rp_export_csv")}</button>
        <button className={cmd} onClick={() => save("pdf")} disabled={!rep}><FileText />{t("rp_export_pdf")}</button>
        {picking && (
          <PeriodPicker
            preset={preset}
            days={days}
            onPreset={(p) => { setPreset(p); setDays(presetDays(p, new Date()).map(dayKey) as [string, string]); setPicking(false); }}
            onDays={(d) => { setPreset("custom"); setDays(d); }}
            onClose={() => setPicking(false)}
          />
        )}
      </div>
      {!valid && <p className="px-4 pt-2 text-sm text-red-500">{t("rp_bad_range")}</p>}

      <div className="flex flex-1 flex-wrap items-stretch">
        <nav aria-label={t("rp_sections")} className="min-w-0 flex-[1_1_210px] border-e line bg-[var(--panel-2)] py-2.5 md:max-w-[240px]">
          <SectionLink on={section === "overview"} onClick={() => setSection("overview")} label={t("rp_overview")} />
          {GROUPS.map((g) => {
            const list = g.ids.map((id) => tables.find((x) => x.id === id)).filter((x): x is Table => !!x);
            if (!list.length) return null;
            return (
              <div key={g.label}>
                <div className="muted px-4 pb-1 pt-3 text-xs font-bold">{t(g.label)}</div>
                {list.map((x) => <SectionLink key={x.id} on={section === x.id} onClick={() => setSection(x.id)} label={x.title} />)}
              </div>
            );
          })}
        </nav>

        <section aria-label={t("rp_title")} className="flex min-w-0 flex-[999_1_640px] flex-col gap-4 p-4 sm:p-5">
          {!rep || !prev ? (
            <p className="muted">{t("loading")}</p>
          ) : shown ? (
            <>
              <h1 className="text-2xl font-semibold">{shown.title}</h1>
              <ReportTable table={shown} lang={t.lang} empty={t("rp_nothing")} />
            </>
          ) : (
            <Overview rep={rep} prev={prev} single={days[0] === days[1]} live={live} onProducts={() => setSection("by_product")} />
          )}
        </section>
      </div>
    </div>
  );
}

function SectionLink({ on, onClick, label }: { on: boolean; onClick: () => void; label: string }) {
  return (
    <button onClick={onClick} aria-current={on ? "page" : undefined}
      className={`mx-1.5 flex min-h-10 w-[calc(100%-12px)] items-center rounded-md px-3 text-start ${on ? "sel sel-mark font-semibold" : "hov"}`}>
      {label}
    </button>
  );
}

function PeriodPicker({ preset, days, onPreset, onDays, onClose }: {
  preset: Preset | "custom"; days: [string, string]; onPreset: (p: Preset) => void; onDays: (d: [string, string]) => void; onClose: () => void;
}) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const c = (e: MouseEvent) => ref.current && !ref.current.contains(e.target as Node) && !(e.target as HTMLElement).closest("[data-testid=rp-period]") && onClose();
    window.addEventListener("keydown", k);
    window.addEventListener("mousedown", c);
    return () => {
      window.removeEventListener("keydown", k);
      window.removeEventListener("mousedown", c);
    };
  }, [onClose]);
  return (
    <div ref={ref} role="dialog" aria-label={t("rp_period")} className="card absolute start-4 top-full z-30 mt-1 w-[min(420px,calc(100vw-32px))] rounded-3xl p-3 shadow-2xl">
      <div className="grid grid-cols-2 gap-1">
        {PRESETS.map((p) => (
          <button key={p} onClick={() => onPreset(p)} aria-pressed={preset === p}
            className={`min-h-11 rounded-md px-3 text-start ${preset === p ? "sel sel-mark font-semibold" : "hov"}`}>
            {t(`rp_preset_${p}` as Key)}
          </button>
        ))}
      </div>
      <div className="mt-3 grid grid-cols-2 gap-2 border-t line pt-3">
        <Field label={t("rp_from")}>
          <TextInput type="date" value={days[0]} max={days[1]} onChange={(e) => onDays([e.target.value, days[1]])} />
        </Field>
        <Field label={t("rp_to")}>
          <TextInput type="date" value={days[1]} min={days[0]} onChange={(e) => onDays([days[0], e.target.value])} />
        </Field>
      </div>
      <div className="mt-3 flex justify-end"><Button variant="primary" onClick={onClose}>{t("close")}</Button></div>
    </div>
  );
}

// ------------------------------------------------------------------ overview

type Tone = "good" | "bad" | "flat";
const toneCls: Record<Tone, string> = {
  good: "bg-[#f1faf1] text-[#0e700e] dark:bg-emerald-500/15 dark:text-emerald-300",
  bad: "bg-[#fdf3f4] text-[#b10e1c] dark:bg-red-500/15 dark:text-red-300",
  flat: "bg-[var(--panel-2)] muted",
};

function Kpi({ label, value, delta, tone, before }: { label: string; value: string; delta: string; tone: Tone; before: string }) {
  return (
    <div className="card rounded-2xl px-4 py-3.5" data-testid="rp-kpi">
      <div className="muted text-[13px]">{label}</div>
      <div className="num mt-1 text-[26px] font-semibold leading-tight">{pctLtr(value)}</div>
      <div className="mt-1.5 flex flex-wrap items-center gap-1.5 text-xs">
        <span className={`rounded-full px-2 py-px font-semibold ${toneCls[tone]}`}>{delta}</span>
        <span className="muted">{before}</span>
      </div>
    </div>
  );
}

function Overview({ rep, prev, single, live, onProducts }: { rep: Report; prev: Report; single: boolean; live: boolean; onProducts: () => void }) {
  const t = useT();
  const m = (c: number) => formatMoney(c, t.lang);
  const a = rep.totals;
  const b = prev.totals;
  // ▲ 12 % / ▼ 3 % / = ; "up is good" except where noted.
  const pct = (x: number, y: number, upGood = true): { delta: string; tone: Tone } => {
    if (x === y) return { delta: "=", tone: "flat" };
    if (!y) return { delta: x > y ? "▲" : "▼", tone: (x > y) === upGood ? "good" : "bad" };
    const p = Math.round(((x - y) / Math.abs(y)) * 100);
    if (p === 0) return { delta: "≈", tone: "flat" };
    return { delta: `${p > 0 ? "▲" : "▼"} ${Math.abs(p)} %`, tone: (p > 0) === upGood ? "good" : "bad" };
  };
  const count = (x: number, y: number): { delta: string; tone: Tone } =>
    x === y ? { delta: "=", tone: "flat" } : { delta: `${x > y ? "▲" : "▼"} ${Math.abs(x - y)}`, tone: x > y ? "good" : "bad" };
  const fcA = rep.food_cost.food_cost_bp;
  const fcB = prev.food_cost.food_cost_bp;
  const fc: { delta: string; tone: Tone } = fcA === null || fcB === null || fcA === fcB
    ? { delta: "=", tone: "flat" }
    : { delta: `${fcA > fcB ? "▲" : "▼"} ${t("rp_points", { v: formatPercent(Math.abs(fcA - fcB), t.lang).replace(" %", "") })}`, tone: fcA > fcB ? "bad" : "good" };
  const before = (v: string) => t("rp_before", { v });
  const dr = a.discount_cents + a.refunded_cents;
  const drPrev = b.discount_cents + b.refunded_cents;

  // Chart: hours for one day, days for a longer period; the previous period in a light shade.
  const chart = single
    ? (() => {
        const act = rep.by_hour.concat(prev.by_hour).filter((h) => h.orders || h.total_cents).map((h) => h.hour);
        const lo = Math.min(11, ...act);
        const hi = Math.max(lo + 11, ...act);
        return Array.from({ length: Math.min(23, hi) - lo + 1 }, (_, i) => lo + i).map((h) => ({
          label: String(h).padStart(2, "0"),
          now: rep.by_hour.find((x) => x.hour === h)?.total_cents ?? 0,
          before: prev.by_hour.find((x) => x.hour === h)?.total_cents ?? 0,
        }));
      })()
    : (() => {
        const n = rep.by_day.length;
        const start = new Date(rep.range.from);
        return Array.from({ length: Math.max(n, Math.round((new Date(rep.range.to).getTime() - start.getTime()) / 86_400_000)) }, (_, i) => {
          const d = addDays(start, i);
          const key = dayKey(d);
          const pk = dayKey(addDays(new Date(prev.range.from), i));
          return {
            label: String(d.getDate()),
            now: rep.by_day.find((x) => x.day === key)?.total_cents ?? 0,
            before: prev.by_day.find((x) => x.day === pk)?.total_cents ?? 0,
          };
        });
      })();
  const max = Math.max(1, ...chart.flatMap((c) => [c.now, c.before]));
  const thisLabel = single && live ? t("rp_preset_today") : t("rp_this_period");
  const prevLabel = single && live ? t("rp_preset_yesterday") : t("rp_prev_period");

  const top = [...rep.by_product].sort((x, y) => y.total_cents - x.total_cents || y.qty - x.qty).slice(0, 5);
  const topMax = Math.max(1, ...top.map((p) => p.total_cents));
  const COLORS = ["#0f6cbd", "#e8a33d", "#7f56b3", "#2b8a3e"];
  const splits = [
    { title: t("rp_by_payment"), parts: rep.by_payment.map((p) => ({ label: t(p.method === "cash" ? "pos_cash" : "pos_card"), value: p.net_cents })) },
    { title: t("rp_by_type"), parts: rep.by_type.map((x) => ({ label: t(`order_type_${x.type}` as Key), value: x.total_cents })) },
  ];

  return (
    <>
      <h1 className="text-2xl font-semibold">{t("rp_overview")}</h1>
      <div className="grid grid-cols-[repeat(auto-fit,minmax(155px,1fr))] gap-3">
        <Kpi label={t("rp_net")} value={m(a.net_cents)} {...pct(a.net_cents, b.net_cents)} before={before(m(b.net_cents))} />
        <Kpi label={t("rp_orders")} value={String(a.orders)} {...count(a.orders, b.orders)} before={before(String(b.orders))} />
        <Kpi label={t("rp_avg_ticket")} value={m(a.avg_ticket_cents)} {...pct(a.avg_ticket_cents, b.avg_ticket_cents)} before={before(m(b.avg_ticket_cents))} />
        <Kpi label={t("rp_food_cost")} value={fcA === null ? "—" : formatPercent(fcA, t.lang)} {...fc} before={before(fcB === null ? "—" : formatPercent(fcB, t.lang))} />
        <Kpi label={t("rp_discounts_refunds")} value={m(dr)} {...pct(dr, drPrev, false)} before={before(m(drPrev))} />
      </div>

      <section className="card rounded-2xl px-5 py-4">
        <div className="flex flex-wrap items-baseline justify-between gap-2">
          <h2 className="text-lg font-semibold">{single ? t("rp_by_hour") : t("rp_by_day")}</h2>
          <span className="muted flex gap-4 text-xs">
            <span className="flex items-center gap-1.5"><span className="h-3 w-3 rounded-sm bg-brand" />{thisLabel}</span>
            <span className="flex items-center gap-1.5"><span className="h-3 w-3 rounded-sm bg-[#c7d7ea] dark:bg-[#3a5068]" />{prevLabel}</span>
          </span>
        </div>
        <div className="mt-3 flex h-48 items-end gap-1.5 border-b line-strong" dir="ltr" role="img" aria-label={single ? t("rp_by_hour") : t("rp_by_day")}>
          {chart.map((c) => (
            <div key={c.label} className="flex h-full min-w-0 flex-1 items-end justify-center gap-[2px]"
              title={`${c.label} · ${thisLabel} ${m(c.now)} · ${prevLabel} ${m(c.before)}`}>
              <span className="block w-[42%] rounded-t-sm bg-[#c7d7ea] dark:bg-[#3a5068]" style={{ height: `${(Math.max(0, c.before) / max) * 100}%` }} />
              <span className="block w-[42%] rounded-t-sm bg-brand" style={{ height: `${(Math.max(0, c.now) / max) * 100}%` }} />
            </div>
          ))}
        </div>
        <div className="muted mt-1 flex gap-1.5 text-[11px]" dir="ltr">
          {chart.map((c, i) => <span key={c.label} className="num min-w-0 flex-1 text-center">{chart.length > 16 && i % 2 ? "" : c.label}</span>)}
        </div>
      </section>

      <div className="flex flex-wrap gap-4">
        <section className="card min-w-0 flex-[2_1_420px] rounded-2xl px-5 py-4">
          <div className="flex items-baseline justify-between gap-2">
            <h2 className="text-lg font-semibold">{t("rp_top_products")}</h2>
            <button onClick={onProducts} className="font-semibold text-brand hover:underline">{t("rp_all_products")}</button>
          </div>
          {top.length === 0 ? <p className="muted mt-3">{t("rp_no_sales")}</p> : (
            <table className="mt-2 w-full text-sm" data-testid="rp-top">
              <thead>
                <tr>
                  <th className="text-start">{t("rp_product")}</th>
                  <th className="px-2 text-end">{t("rp_qty")}</th>
                  <th className="px-2 text-end">{t("rp_net")}</th>
                  <th className="text-end">{t("rp_food_cost")}</th>
                </tr>
              </thead>
              <tbody>
                {top.map((p) => (
                  <tr key={p.product_id}>
                    <td>
                      <div className="font-semibold">{pickName(p.names ?? undefined, t.lang)}</div>
                      <div className="mt-1 h-1.5 w-full rounded-full bg-[var(--panel-2)]"><div className="h-1.5 rounded-full bg-brand" style={{ width: `${(p.total_cents / topMax) * 100}%` }} /></div>
                    </td>
                    <td className="num px-2 text-end">{p.qty}</td>
                    <td className="num whitespace-nowrap px-2 text-end">{m(p.total_cents)}</td>
                    <td className="num text-end"><span dir="ltr">{p.food_cost_bp === null ? "—" : formatPercent(p.food_cost_bp, t.lang)}</span></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </section>

        <section className="card flex min-w-0 flex-[1_1_280px] flex-col gap-4 rounded-2xl px-5 py-4">
          <h2 className="text-lg font-semibold">{t("rp_breakdown")}</h2>
          {splits.map((sp) => {
            const total = sp.parts.reduce((s, x) => s + Math.max(0, x.value), 0);
            const share = (v: number) => (total ? Math.round((Math.max(0, v) / total) * 100) : 0);
            return (
              <div key={sp.title}>
                <div className="muted mb-1.5 text-xs font-bold">{sp.title}</div>
                {!sp.parts.length ? <p className="muted text-sm">{t("rp_no_sales")}</p> : (
                  <>
                    <div className="flex h-3 overflow-hidden rounded-full bg-[var(--panel-2)]">
                      {sp.parts.map((x, i) => <span key={x.label} style={{ width: `${share(x.value)}%`, background: COLORS[i % COLORS.length] }} />)}
                    </div>
                    {sp.parts.map((x, i) => (
                      <div key={x.label} className="flex items-center gap-2 border-b line py-1.5 last:border-b-0">
                        <span className="h-2.5 w-2.5 rounded-sm" style={{ background: COLORS[i % COLORS.length] }} />
                        <span className="min-w-0 flex-1 truncate">{x.label}</span>
                        <span className="num muted" dir="ltr">{share(x.value)} %</span>
                        <b className="num w-28 text-end">{m(x.value)}</b>
                      </div>
                    ))}
                  </>
                )}
              </div>
            );
          })}
        </section>
      </div>
    </>
  );
}

// ------------------------------------------------------------------ one section as a data grid

function ReportTable({ table, lang, empty }: { table: Table; lang: Lang; empty: string }) {
  return (
    <Card>
      {table.rows.length === 0 ? (
        <p className="muted">{empty}</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm" data-report={table.id}>
            <thead>
              <tr>
                {table.columns.map((c, i) => (
                  <th key={i} className={`pe-3 ${c.num ? "text-end" : "text-start"}`}>{c.label}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {table.rows.map((r, i) => (
                <tr key={i}>
                  {r.map((c, j) => (
                    <td key={j} className={`pe-3 ${isNum(c) ? "num whitespace-nowrap text-end" : ""}`}>{pctLtr(cellText(c, lang))}</td>
                  ))}
                </tr>
              ))}
            </tbody>
            {table.total && (
              <tfoot>
                <tr className="border-t-2 line-strong font-semibold">
                  {table.total.map((c, j) => (
                    <td key={j} className={`py-2.5 pe-3 ${isNum(c) ? "num whitespace-nowrap text-end" : ""}`}>{pctLtr(cellText(c, lang))}</td>
                  ))}
                </tr>
              </tfoot>
            )}
          </table>
        </div>
      )}
      {table.note && <p className="muted mt-3 text-xs">{table.note}</p>}
    </Card>
  );
}
