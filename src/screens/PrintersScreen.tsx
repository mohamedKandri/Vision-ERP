import { Plus, Printer as PrinterIcon, RotateCw } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { useApproval } from "../components/PosDialogs";
import { Button, Card, Field, Modal, Select, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { pickName, type Names } from "../lib/menu";
import { isApiError, rpc } from "../lib/rpc";
import { getRegister } from "./PosScreen";

type Kind = "network" | "bluetooth" | "usb" | "sunmi" | "http";
interface PrinterRow {
  id: number;
  name: string;
  kind: Kind;
  address: string;
  paper_mm: 58 | 80;
  codepage: string;
  graphic_mode: boolean;
  cut: boolean;
  drawer: boolean;
  receipt_register: string | null;
  ticket_lang: "fr" | "en";
  active: boolean;
  format: "raw" | "base64";
  http_field: string;
  has_http_auth: boolean;
  /** Only sent when typed: the stored secret never comes back to the screen. */
  http_auth?: string;
  last_ok_at: string | null;
  last_error: string | null;
  pending: number;
  failed: number;
}
interface StationRow {
  id: number;
  code: string;
  names: Names;
  printer_id: number | null;
  show_on_screen: boolean;
}
interface Job {
  id: number;
  printer_id: number;
  printer: string;
  kind: string;
  title: string;
  status: string;
  attempts: number;
  last_error: string | null;
  created_at: string;
}

export function PrintersScreen() {
  const t = useT();
  const toast = useToast();
  const approval = useApproval();
  const register = getRegister();
  const [data, setData] = useState<{ printers: PrinterRow[]; stations: StationRow[] } | null>(null);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [edit, setEdit] = useState<Partial<PrinterRow> | null>(null);
  const [discover, setDiscover] = useState<{ usb: string[] | null; usb_error: string | null; bluetooth: { name: string; address: string }[] | null; bluetooth_error: string | null } | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await rpc("printers.list"));
      setJobs(await rpc<Job[]>("print.jobs", { scope: "open" }));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => {
    void load();
    const id = setInterval(load, 4000);
    return () => clearInterval(id);
  }, [load]);

  const fail = (e: unknown) => !(isApiError(e) && e.code === "cancelled") && toast.error(errorText(t.lang, e));

  const openEditor = async (p: Partial<PrinterRow>) => {
    setEdit(p);
    rpc<typeof discover>("printers.discover").then(setDiscover, () => {});
  };

  const save = async () => {
    if (!edit) return;
    try {
      await rpc("printers.save", {
        id: edit.id ?? null, name: edit.name ?? "", kind: edit.kind ?? "network", address: edit.address ?? "",
        paper_mm: edit.paper_mm ?? 80, codepage: edit.codepage ?? "cp850", graphic_mode: !!edit.graphic_mode, cut: edit.cut ?? true,
        drawer: !!edit.drawer, receipt_register: edit.receipt_register ?? null, ticket_lang: edit.ticket_lang ?? "fr", active: edit.active ?? true,
        format: edit.kind === "http" ? edit.format ?? "raw" : "raw", http_field: edit.http_field || "data",
        http_auth: edit.http_auth === undefined ? null : edit.http_auth,
      });
      setEdit(null);
      toast.ok(t("saved"));
      load();
    } catch (e) {
      fail(e);
    }
  };

  const test = async (id: number) => {
    try {
      const { job_id } = await rpc<{ job_id: number }>("printers.test", { id });
      for (let i = 0; i < 20; i++) {
        await new Promise((r) => setTimeout(r, 500));
        const st = await rpc<{ status: string; last_error: string | null }>("print.job", { id: job_id });
        if (st.status === "done") return toast.ok(t("pr_test_ok"));
        if (st.status === "failed") return toast.error(t("pr_test_fail", { e: st.last_error ?? "?" }));
      }
    } catch (e) {
      fail(e);
    } finally {
      load();
    }
  };

  if (!data) return <p className="muted p-6">{t("loading")}</p>;
  const active = data.printers.filter((p) => p.active);
  const kinds: Kind[] = ["network", "bluetooth", "usb", "sunmi", "http"];

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-4 p-4">
      <Card title={t("nav_printers")} actions={
        <Button variant="primary" onClick={() => openEditor({ kind: "network", paper_mm: 80, codepage: "cp850", cut: true, active: true, ticket_lang: "fr" })}>
          <Plus className="inline h-4 w-4" /> {t("pr_add")}
        </Button>
      }>
        <div className="flex flex-col divide-y line">
          {data.printers.map((p) => (
            <div key={p.id} className={`flex flex-wrap items-center gap-3 py-3 ${p.active ? "" : "opacity-50"}`}>
              <PrinterIcon className="shrink-0" />
              <button className="min-w-0 flex-1 text-start" onClick={() => openEditor(p)}>
                <div className="font-semibold">
                  {p.name}
                  {p.receipt_register && <span className="ms-2 rounded bg-brand/20 px-1.5 text-xs">🧾 {p.receipt_register}</span>}
                </div>
                <div className="muted truncate text-sm">{t(`pr_kind_${p.kind}` as Key)}{p.kind === "http" ? ` (${t(`pr_format_${p.format}` as Key)})` : ""} · {p.address || "InnerPrinter"} · {p.paper_mm} mm</div>
                <div className={`text-xs ${p.last_error ? "text-red-500" : p.last_ok_at ? "text-emerald-500" : "muted"}`}>
                  {p.last_error ? `${t("pr_error")}: ${p.last_error}` : p.last_ok_at ? t("pr_ok") : t("pr_never")}
                  {p.pending + p.failed > 0 && ` · ${t("pr_pending", { n: p.pending + p.failed })}`}
                </div>
              </button>
              {p.failed > 0 && <Button onClick={() => rpc("print.retry", { printer_id: p.id }).then(load, fail)}><RotateCw className="inline h-4 w-4" /> {t("pr_retry")}</Button>}
              <Button onClick={() => test(p.id)} disabled={!p.active}>{t("pr_test")}</Button>
            </div>
          ))}
        </div>
      </Card>

      <Card title={t("pr_routing")}>
        <p className="muted mb-3 text-sm">{t("pr_routing_hint")}</p>
        <div className="flex flex-col gap-2">
          {data.stations.map((s) => (
            <div key={s.id} className="grid items-center gap-2 sm:grid-cols-[10rem_1fr_auto]">
              <span className="font-semibold">{pickName(s.names, t.lang)}</span>
              <Select<number>
                value={s.printer_id ?? 0}
                onChange={(v) => rpc("stations.routing", { station_id: s.id, printer_id: v || null, show_on_screen: v ? s.show_on_screen : true }).then(load, fail)}
                options={[{ value: 0, label: t("pr_no_printer") }, ...active.map((p) => ({ value: p.id, label: p.name }))]}
              />
              <Toggle checked={s.show_on_screen} label={t("pr_screen")}
                onChange={(v) => rpc("stations.routing", { station_id: s.id, printer_id: s.printer_id, show_on_screen: v }).then(load, fail)} />
            </div>
          ))}
        </div>
      </Card>

      {jobs.length > 0 && (
        <Card title={t("pr_jobs")}>
          <div className="flex flex-col divide-y line">
            {jobs.map((j) => (
              <div key={j.id} className="flex flex-wrap items-center gap-2 py-2">
                <span className="min-w-0 flex-1">
                  <span className="font-medium">{j.title || j.kind}</span> <span className="muted text-sm">→ {j.printer}</span>
                  {j.last_error && <span className="block text-xs text-red-500">{j.last_error} ({j.attempts}×)</span>}
                </span>
                <Button onClick={() => rpc("print.retry", { job_id: j.id }).then(load, fail)}>{t("pr_retry")}</Button>
                <Button variant="danger" onClick={() => approval.ask((pin) => rpc("print.cancel", { job_id: j.id, approver_pin: pin })).then(load, fail)}>{t("pr_cancel_job")}</Button>
              </div>
            ))}
          </div>
          {data.printers.some((p) => p.failed > 0) && active.length > 1 && (
            <div className="mt-3 flex flex-wrap items-center gap-2">
              <span>{t("pr_reroute")}:</span>
              {data.printers.filter((p) => p.failed > 0).map((from) =>
                active.filter((x) => x.id !== from.id).map((to) => (
                  <Button key={`${from.id}-${to.id}`} onClick={() => approval.ask((pin) => rpc("print.reroute", { from: from.id, to: to.id, approver_pin: pin })).then(load, fail)}>
                    {from.name} → {to.name}
                  </Button>
                )),
              )}
            </div>
          )}
        </Card>
      )}

      {edit && (
        <Modal onClose={() => setEdit(null)} wide>
          <div className="flex flex-col gap-3">
            <Field label={t("name")}><TextInput autoFocus value={edit.name ?? ""} onChange={(e) => setEdit({ ...edit, name: e.target.value })} placeholder="Grill, Comptoir…" /></Field>
            <Field label={t("pr_kind")}>
              <div className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                {kinds.map((k) => (
                  <button key={k} onClick={() => setEdit({ ...edit, kind: k, address: k === "sunmi" ? "" : edit.kind === k ? edit.address : "", drawer: k === "sunmi" ? true : edit.drawer, paper_mm: k === "sunmi" ? 58 : edit.paper_mm })}
                    className={`min-h-12 rounded-xl border px-2 text-sm font-semibold ${edit.kind === k ? "border-brand bg-brand/20" : "line panel-2"}`}>
                    {t(`pr_kind_${k}` as Key)}
                  </button>
                ))}
              </div>
            </Field>
            {edit.kind === "network" && (
              <Field label={t("pr_address")}><TextInput dir="ltr" inputMode="url" placeholder="192.168.1.50" value={edit.address ?? ""} onChange={(e) => setEdit({ ...edit, address: e.target.value })} /></Field>
            )}
            {edit.kind === "bluetooth" && (
              <Field label={t("pr_bt_device")} hint={discover?.bluetooth_error ? t("pr_bt_unavailable") : undefined}>
                {discover?.bluetooth?.length ? (
                  <Select<string> value={edit.address ?? ""} onChange={(v) => setEdit({ ...edit, address: v })}
                    options={[{ value: "", label: "—" }, ...discover.bluetooth.map((d) => ({ value: d.address, label: `${d.name} (${d.address})` }))]} />
                ) : (
                  <TextInput dir="ltr" placeholder="00:11:22:AA:BB:CC" value={edit.address ?? ""} onChange={(e) => setEdit({ ...edit, address: e.target.value })} />
                )}
              </Field>
            )}
            {edit.kind === "usb" && (
              <Field label={t("pr_usb_device")} hint={discover?.usb_error ? t("pr_usb_unavailable") : undefined}>
                {discover?.usb?.length ? (
                  <Select<string> value={edit.address ?? ""} onChange={(v) => setEdit({ ...edit, address: v })} options={[{ value: "", label: "—" }, ...discover.usb.map((n) => ({ value: n, label: n }))]} />
                ) : (
                  <TextInput value={edit.address ?? ""} onChange={(e) => setEdit({ ...edit, address: e.target.value })} />
                )}
              </Field>
            )}
            {edit.kind === "sunmi" && <p className="muted text-sm">{t("pr_sunmi_hint")}</p>}
            {edit.kind === "http" && (
              <>
                <Field label={t("pr_url")}><TextInput dir="ltr" inputMode="url" placeholder="http://192.168.1.60:8080/print" value={edit.address ?? ""} onChange={(e) => setEdit({ ...edit, address: e.target.value })} /></Field>
                <Field label={t("pr_format")}>
                  <Select<string> value={edit.format ?? "raw"} onChange={(v) => setEdit({ ...edit, format: v as "raw" | "base64" })}
                    options={[{ value: "raw", label: t("pr_format_raw") }, { value: "base64", label: t("pr_format_base64") }]} />
                </Field>
                {edit.format === "base64" && (
                  <Field label={t("pr_http_field")}><TextInput dir="ltr" placeholder="data" value={edit.http_field ?? "data"} onChange={(e) => setEdit({ ...edit, http_field: e.target.value })} /></Field>
                )}
                <Field label={t("pr_http_auth")}>
                  <TextInput dir="ltr" type="password" autoComplete="off" placeholder={edit.has_http_auth ? "••••••" : "Bearer …"} value={edit.http_auth ?? ""} onChange={(e) => setEdit({ ...edit, http_auth: e.target.value })} />
                </Field>
                <p className="muted text-sm">{t(edit.format === "base64" ? "pr_http_hint_base64" : "pr_http_hint_raw")}</p>
              </>
            )}
            <div className="grid gap-3 sm:grid-cols-3">
              <Field label={t("pr_paper")}>
                <Select<number> value={edit.paper_mm ?? 80} onChange={(v) => setEdit({ ...edit, paper_mm: v as 58 | 80 })} options={[{ value: 58, label: "58 mm" }, { value: 80, label: "80 mm" }]} />
              </Field>
              <Field label={t("pr_codepage")}>
                <Select<string> value={edit.codepage ?? "cp850"} onChange={(v) => setEdit({ ...edit, codepage: v })}
                  options={[["cp850", "PC850 (Europe)"], ["cp858", "PC858 (Europe + €)"], ["cp1252", "Windows-1252"], ["cp437", "PC437 (US)"], ["ascii", "ASCII (sans accents)"]].map(([value, label]) => ({ value, label }))} />
              </Field>
              <Field label={t("pr_ticket_lang")}>
                <Select<string> value={edit.ticket_lang ?? "fr"} onChange={(v) => setEdit({ ...edit, ticket_lang: v as "fr" | "en" })} options={[{ value: "fr", label: "Français" }, { value: "en", label: "English" }]} />
              </Field>
            </div>
            <Toggle checked={edit.receipt_register === register} onChange={(v) => setEdit({ ...edit, receipt_register: v ? register : null })} label={`${t("pr_receipt_here")} (${register})`} />
            <Toggle checked={!!edit.drawer} onChange={(v) => setEdit({ ...edit, drawer: v })} label={t("pr_drawer")} />
            <Toggle checked={edit.cut ?? true} onChange={(v) => setEdit({ ...edit, cut: v })} label={t("pr_cut")} />
            <Toggle checked={!!edit.graphic_mode} onChange={(v) => setEdit({ ...edit, graphic_mode: v })} label={t("pr_graphic")} />
            <Toggle checked={edit.active ?? true} onChange={(v) => setEdit({ ...edit, active: v })} label={t("active")} />
            <div className="flex justify-end gap-3">
              <Button onClick={() => setEdit(null)}>{t("cancel")}</Button>
              <Button variant="primary" onClick={save} disabled={!edit.name?.trim()}>{t("save")}</Button>
            </div>
          </div>
        </Modal>
      )}
      {approval.node}
      {toast.node}
    </div>
  );
}

/** Red banner on the till when printers have unprinted jobs. */
export function PrintProblems() {
  const t = useT();
  const [rows, setRows] = useState<{ printer_id: number; name: string; error: string | null; failed: number }[]>([]);
  useEffect(() => {
    const tick = () => rpc<typeof rows>("print.problems").then(setRows, () => {});
    tick();
    const id = setInterval(tick, 5000);
    return () => clearInterval(id);
  }, []);
  if (!rows.length) return null;
  return (
    <div className="flex flex-col gap-1 bg-red-600 px-3 py-2 text-white" role="alert">
      {rows.map((r) => (
        <div key={r.printer_id} className="flex flex-wrap items-center gap-2">
          <span className="flex-1 font-semibold">🖨️ {t("pr_problem", { p: r.name, n: r.failed })}{r.error ? ` — ${r.error}` : ""}</span>
          <button className="min-h-11 rounded-lg bg-white/20 px-3 font-semibold" onClick={() => rpc("print.retry", { printer_id: r.printer_id }).then(() => setRows([]), () => {})}>
            {t("pr_retry")}
          </button>
        </div>
      ))}
    </div>
  );
}
