import { Download, FileText, RefreshCw } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Button, Card, Field, Tabs, TextInput, useToast } from "../components/ui";
import { errorText, translate, useT, type Key } from "../i18n";
import { useLive } from "../lib/live";
import { pickName } from "../lib/menu";
import { formatMoney } from "../lib/money";
import {
  addDays, cellText, dayKey, dayRange, exportName, isNum, parseDay, presetDays, PRESETS, reportTables, startOfDay, toBase64, toCsv,
  type Preset, type RangeIn, type Report, type Table,
} from "../lib/report";
import { renderReportPdf } from "../lib/reportPdf";
import { rpc, type Lang } from "../lib/rpc";

type Tab = "today" | "range";

export function ReportsScreen() {
  const t = useT();
  const [tab, setTab] = useState<Tab>("today");
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4 p-4">
      <Tabs value={tab} onChange={setTab} tabs={[{ id: "today", label: t("rp_tab_today") }, { id: "range", label: t("rp_tab_range") }]} />
      {tab === "today" ? <TodayTab /> : <RangeTab />}
    </div>
  );
}

const tr = (lang: Lang) => (k: string, v?: Record<string, string | number>) => translate(lang, k as Key, v);
const fmtDateTime = (lang: Lang) => {
  const f = new Intl.DateTimeFormat(`${lang === "ar" ? "ar-MA" : lang === "en" ? "en-GB" : "fr-FR"}-u-nu-latn`, { dateStyle: "short", timeStyle: "short" });
  return (iso: string) => f.format(new Date(iso));
};
const fmtDay = (lang: Lang, d: Date) =>
  new Intl.DateTimeFormat(`${lang === "ar" ? "ar-MA" : lang === "en" ? "en-GB" : "fr-FR"}-u-nu-latn`, { dateStyle: "medium" }).format(d);

// ------------------------------------------------------------------ today at a glance

function Kpi({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="panel rounded-2xl p-4">
      <div className="muted text-sm">{label}</div>
      <div className="num mt-1 text-2xl font-semibold">{value}</div>
      {sub && <div className="muted mt-1 text-xs">{sub}</div>}
    </div>
  );
}

function TodayTab() {
  const t = useT();
  const [today, setToday] = useState<Report | null>(null);
  const [yday, setYday] = useState<Report | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const load = useCallback(() => {
    const now = new Date();
    const r = dayRange(now, now);
    // Yesterday up to the same time, for a fair comparison.
    const y0 = addDays(startOfDay(now), -1);
    const y: RangeIn = { from: y0.toISOString(), to: new Date(now.getTime() - 86_400_000).toISOString(), tz_offset_min: -y0.getTimezoneOffset() };
    Promise.all([rpc<Report>("reports.summary", r), rpc<Report>("reports.summary", y)]).then(
      ([a, b]) => {
        setToday(a);
        setYday(b);
        setErr(null);
      },
      (e) => setErr(errorText(t.lang, e)),
    );
  }, [t.lang]);
  useEffect(() => void load(), [load]);
  useLive(["orders", "cash"], load, 30000);

  if (err) return <Card><p className="text-red-500">{err}</p></Card>;
  if (!today || !yday) return <p className="muted p-4">{t("loading")}</p>;
  const m = (c: number) => formatMoney(c, t.lang);
  const tt = today.totals;
  const vs = (a: number, b: number, money: boolean) => t("rp_vs_yesterday", { v: money ? m(b) : String(b) }) + (b ? ` (${a >= b ? "+" : ""}${Math.round(((a - b) / b) * 100)} %)` : "");
  const top = [...today.by_product].sort((a, b) => b.qty - a.qty || b.total_cents - a.total_cents).slice(0, 5);
  return (
    <>
      <div className="grid grid-cols-2 gap-3 lg:grid-cols-4">
        <Kpi label={t("rp_net")} value={m(tt.net_cents)} sub={vs(tt.net_cents, yday.totals.net_cents, true)} />
        <Kpi label={t("rp_orders")} value={String(tt.orders)} sub={vs(tt.orders, yday.totals.orders, false)} />
        <Kpi label={t("rp_avg_ticket")} value={m(tt.avg_ticket_cents)} sub={t("rp_vs_yesterday", { v: m(yday.totals.avg_ticket_cents) })} />
        <Kpi label={t("rp_discounts_refunds")} value={m(tt.discount_cents + tt.refunded_cents)}
          sub={t("rp_discounts_refunds_sub", { d: m(tt.discount_cents), r: m(tt.refunded_cents) })} />
      </div>
      <Card title={t("rp_by_hour")}>
        <HourChart hours={today.by_hour} lang={t.lang} />
      </Card>
      <div className="grid gap-4 lg:grid-cols-3">
        <Card title={t("rp_top_products")}>
          {top.length === 0 ? <p className="muted">{t("rp_no_sales")}</p> : (
            <ol className="flex flex-col gap-2">
              {top.map((p, i) => (
                <li key={p.product_id} className="flex items-baseline gap-2">
                  <span className="muted num w-5">{i + 1}.</span>
                  <span className="flex-1 truncate">{pickName(p.names ?? undefined, t.lang)}</span>
                  <span className="num font-semibold">× {p.qty}</span>
                  <span className="num muted w-28 text-end text-sm">{m(p.total_cents)}</span>
                </li>
              ))}
            </ol>
          )}
        </Card>
        <Card title={t("rp_by_payment")}>
          <MiniList rows={today.by_payment.map((p) => [t(p.method === "cash" ? "pos_cash" : "pos_card"), m(p.net_cents)])} empty={t("rp_no_sales")} />
        </Card>
        <Card title={t("rp_by_type")}>
          <MiniList rows={today.by_type.map((x) => [`${t(`order_type_${x.type}` as Key)} · ${x.orders}`, m(x.total_cents)])} empty={t("rp_no_sales")} />
        </Card>
      </div>
    </>
  );
}

function MiniList({ rows, empty }: { rows: [string, string][]; empty: string }) {
  if (!rows.length) return <p className="muted">{empty}</p>;
  return (
    <ul className="flex flex-col gap-2">
      {rows.map(([a, b]) => (
        <li key={a} className="flex justify-between gap-2">
          <span>{a}</span>
          <span className="num font-semibold">{b}</span>
        </li>
      ))}
    </ul>
  );
}

/** Net sales per local hour. One series: one colour, no legend; hover or tap shows the value. */
function HourChart({ hours, lang }: { hours: Report["by_hour"]; lang: Lang }) {
  const [hover, setHover] = useState<number | null>(null);
  const active = hours.filter((h) => h.orders || h.total_cents).map((h) => h.hour);
  const nowH = new Date().getHours();
  const first = Math.min(active[0] ?? 11, nowH, 11);
  const last = Math.min(23, Math.max(active[active.length - 1] ?? 23, nowH, first + 11)); // at least 12 hours
  const shown = hours.slice(first, last + 1);
  const max = Math.max(1, ...shown.map((h) => h.total_cents));
  const t = (k: Key, v?: Record<string, string | number>) => translate(lang, k, v);
  return (
    <div>
      <div className="flex h-48 items-end gap-1" dir="ltr" role="img" aria-label={t("rp_by_hour")}>
        {shown.map((h) => {
          const pct = Math.max(0, h.total_cents) / max;
          return (
            <button
              key={h.hour}
              type="button"
              className="group relative flex h-full flex-1 flex-col justify-end"
              onMouseEnter={() => setHover(h.hour)}
              onMouseLeave={() => setHover(null)}
              onClick={() => setHover(hover === h.hour ? null : h.hour)}
              aria-label={`${h.hour}:00 ${formatMoney(h.total_cents, lang)}`}
            >
              {hover === h.hour && (
                <span className="panel-2 border line absolute bottom-full start-1/2 z-10 mb-1 -translate-x-1/2 whitespace-nowrap rounded-lg px-2 py-1 text-xs shadow">
                  {String(h.hour).padStart(2, "0")}:00 · {formatMoney(h.total_cents, lang)} · {t("rp_n_orders", { n: h.orders })}
                </span>
              )}
              <span
                className={`mx-auto block w-3/4 max-w-12 rounded-t ${hover === h.hour ? "bg-brand-strong" : "bg-brand"}`}
                style={{ height: `${pct * 100}%`, minHeight: h.total_cents > 0 ? 2 : 0 }}
              />
            </button>
          );
        })}
      </div>
      <div className="muted mt-1 flex gap-1 text-[11px]" dir="ltr">
        {shown.map((h) => (
          <span key={h.hour} className="num flex-1 text-center">{h.hour % 2 === 0 ? String(h.hour).padStart(2, "0") : ""}</span>
        ))}
      </div>
    </div>
  );
}

// ------------------------------------------------------------------ date range

function RangeTab() {
  const t = useT();
  const toast = useToast();
  const [preset, setPreset] = useState<Preset | "custom">("today");
  const [days, setDays] = useState<[string, string]>(() => presetDays("today", new Date()).map(dayKey) as [string, string]);
  const [rep, setRep] = useState<Report | null>(null);
  const [busy, setBusy] = useState(false);
  const [shop, setShop] = useState("");
  const [cats, setCats] = useState<Record<number, string>>({});

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
    rpc<Report>("reports.summary", dayRange(first, last)).then(
      (r) => {
        setRep(r);
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

  const tables = useMemo(() => (rep ? reportTables(rep, tr(t.lang), t.lang, fmtDateTime(t.lang), (id) => cats[id] ?? "") : []), [rep, t.lang, cats]);
  const period = first && last ? (days[0] === days[1] ? fmtDay(t.lang, first) : t("rp_from_to", { a: fmtDay(t.lang, first), b: fmtDay(t.lang, last) })) : "";

  const save = async (ext: "csv" | "pdf") => {
    if (!rep) return;
    try {
      const head = [shop, `${t("rp_title")} · ${period}`, t("rp_generated", { d: fmtDateTime(t.lang)(new Date().toISOString()) })];
      let bytes: Uint8Array;
      if (ext === "csv") {
        bytes = new TextEncoder().encode(toCsv(head, tables, t.lang));
      } else {
        bytes = renderReportPdf(
          { shop, title: t("rp_title"), lines: [period, head[2]], footer: (p, n) => t("rp_page", { p, n }) },
          tables,
          t.lang,
        );
      }
      const r = await rpc<{ path: string }>("device.save_export", { name: exportName(t("rp_file_prefix"), rep.range, ext), data: toBase64(bytes) });
      toast.ok(t("rp_saved", { p: r.path }));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  return (
    <>
      {toast.node}
      <Card>
        <div className="flex flex-wrap gap-2">
          {PRESETS.map((p) => (
            <Button
              key={p}
              variant={preset === p ? "primary" : "ghost"}
              onClick={() => {
                setPreset(p);
                setDays(presetDays(p, new Date()).map(dayKey) as [string, string]);
              }}
            >
              {t(`rp_preset_${p}` as Key)}
            </Button>
          ))}
        </div>
        <div className="mt-3 grid gap-3 sm:grid-cols-[1fr_1fr_auto_auto_auto] sm:items-end">
          <Field label={t("rp_from")}>
            <TextInput type="date" value={days[0]} max={days[1]} onChange={(e) => { setPreset("custom"); setDays([e.target.value, days[1]]); }} />
          </Field>
          <Field label={t("rp_to")}>
            <TextInput type="date" value={days[1]} min={days[0]} onChange={(e) => { setPreset("custom"); setDays([days[0], e.target.value]); }} />
          </Field>
          <Button onClick={load} disabled={!valid || busy} aria-label={t("rp_refresh")}><RefreshCw className={busy ? "animate-spin" : ""} /></Button>
          <Button onClick={() => save("csv")} disabled={!rep}><Download className="inline" /> CSV</Button>
          <Button onClick={() => save("pdf")} disabled={!rep}><FileText className="inline" /> PDF</Button>
        </div>
        {!valid && <p className="mt-2 text-sm text-red-500">{t("rp_bad_range")}</p>}
        {period && <p className="muted mt-2 text-sm">{period}</p>}
      </Card>
      {!rep ? <p className="muted p-4">{t("loading")}</p> : tables.map((tb) => <ReportTable key={tb.id} table={tb} lang={t.lang} empty={t("rp_nothing")} />)}
    </>
  );
}

function ReportTable({ table, lang, empty }: { table: Table; lang: Lang; empty: string }) {
  return (
    <Card title={table.title}>
      {table.rows.length === 0 ? (
        <p className="muted">{empty}</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-sm" data-report={table.id}>
            <thead className="muted">
              <tr>
                {table.columns.map((c, i) => (
                  <th key={i} className={`py-2 pe-3 font-medium ${c.num ? "text-end" : "text-start"}`}>{c.label}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {table.rows.map((r, i) => (
                <tr key={i} className="border-t line">
                  {r.map((c, j) => (
                    <td key={j} className={`py-2 pe-3 ${isNum(c) ? "num text-end whitespace-nowrap" : ""}`}>{cellText(c, lang)}</td>
                  ))}
                </tr>
              ))}
            </tbody>
            {table.total && (
              <tfoot>
                <tr className="border-t-2 line font-semibold">
                  {table.total.map((c, j) => (
                    <td key={j} className={`py-2 pe-3 ${isNum(c) ? "num text-end whitespace-nowrap" : ""}`}>{cellText(c, lang)}</td>
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
