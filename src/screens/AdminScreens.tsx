import { Download, Plus } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { Button, Card, Field, Modal, Select, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, LANG_NAMES, useT, type Key } from "../i18n";
import { formatRate } from "../lib/money";
import { addDays, dayKey, parseDay, toBase64, toCsv, type Table } from "../lib/report";
import { rpc, type Lang, type SessionUser, type TaxRow } from "../lib/rpc";

const ROLES = ["owner", "manager", "cashier", "kitchen"] as const;
const langOptions = (Object.keys(LANG_NAMES) as Lang[]).map((l) => ({ value: l, label: LANG_NAMES[l] }));

// ---------------------------------------------------------------- Taxes

export function TaxesScreen({ canEdit }: { canEdit: boolean }) {
  const t = useT();
  const toast = useToast();
  const [rows, setRows] = useState<TaxRow[]>([]);
  const [edit, setEdit] = useState<Partial<TaxRow> | null>(null);
  const load = useCallback(() => rpc<TaxRow[]>("taxes.list").then(setRows), []);
  useEffect(() => void load(), [load]);

  const save = async () => {
    if (!edit) return;
    try {
      await rpc("taxes.save", { id: edit.id ?? null, name: edit.name ?? "", rate_bp: edit.rate_bp ?? 0, price_include: edit.price_include ?? true, active: edit.active ?? true });
      setEdit(null);
      toast.ok(t("saved"));
      load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  return (
    <div className="mx-auto max-w-3xl p-4">
      <Card
        title={t("nav_taxes")}
        actions={canEdit && <Button variant="primary" onClick={() => setEdit({ price_include: true, active: true, rate_bp: 1000 })}><Plus className="inline" /> {t("add")}</Button>}
      >
        <div className="flex flex-col divide-y line">
          {rows.map((r) => (
            <button key={r.id} disabled={!canEdit} onClick={() => setEdit(r)} className={`flex min-h-14 items-center justify-between gap-3 px-1 text-start ${r.active ? "" : "opacity-50"}`}>
              <span className="font-medium">{r.name}</span>
              <span className="muted">
                {formatRate(r.rate_bp)} · {r.price_include ? t("incl") : t("excl")}
              </span>
            </button>
          ))}
        </div>
      </Card>
      {edit && (
        <Modal onClose={() => setEdit(null)}>
          <div className="flex flex-col gap-3">
            <Field label={t("name")}>
              <TextInput value={edit.name ?? ""} onChange={(e) => setEdit({ ...edit, name: e.target.value })} autoFocus />
            </Field>
            <Field label={t("tax_rate")}>
              <TextInput type="number" inputMode="decimal" step={0.5} min={0} max={100} value={(edit.rate_bp ?? 0) / 100} onChange={(e) => setEdit({ ...edit, rate_bp: Math.round(Number(e.target.value) * 100) })} />
            </Field>
            <Toggle checked={edit.price_include ?? true} onChange={(v) => setEdit({ ...edit, price_include: v })} label={t("price_include")} />
            <p className="muted text-sm">{t("price_include_hint")}</p>
            <Toggle checked={edit.active ?? true} onChange={(v) => setEdit({ ...edit, active: v })} label={t("active")} />
            <div className="mt-2 flex justify-end gap-3">
              <Button onClick={() => setEdit(null)}>{t("cancel")}</Button>
              <Button variant="primary" onClick={save} disabled={!edit.name?.trim()}>{t("save")}</Button>
            </div>
          </div>
        </Modal>
      )}
      {toast.node}
    </div>
  );
}

// ---------------------------------------------------------------- Users

interface UserRow {
  id: number;
  name: string;
  role: SessionUser["role"];
  language: Lang;
  theme: "dark" | "light";
  active: boolean;
}

export function UsersScreen({ me }: { me: SessionUser }) {
  const t = useT();
  const toast = useToast();
  const [rows, setRows] = useState<UserRow[]>([]);
  const [edit, setEdit] = useState<(Partial<UserRow> & { pin?: string }) | null>(null);
  const load = useCallback(() => rpc<UserRow[]>("users.list").then(setRows), []);
  useEffect(() => void load(), [load]);

  const save = async () => {
    if (!edit) return;
    try {
      await rpc("users.save", {
        id: edit.id ?? null, name: edit.name ?? "", role: edit.role ?? "cashier", pin: edit.pin || null,
        language: edit.language ?? "fr", theme: edit.theme ?? "dark", active: edit.active ?? true,
      });
      setEdit(null);
      toast.ok(t("saved"));
      load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  const roleOptions = ROLES.filter((r) => r !== "owner" || me.role === "owner").map((r) => ({ value: r, label: t(`role_${r}`) }));
  const isNew = edit && edit.id === undefined;
  const pinOk = edit && (edit.pin ? /^\d{4,6}$/.test(edit.pin) : !isNew);

  return (
    <div className="mx-auto max-w-3xl p-4">
      <Card title={t("nav_users")} actions={<Button variant="primary" onClick={() => setEdit({ role: "cashier", language: t.lang, theme: "dark", active: true })}><Plus className="inline" /> {t("add")}</Button>}>
        <div className="flex flex-col divide-y line">
          {rows.map((u) => (
            <button key={u.id} onClick={() => setEdit({ ...u })} disabled={u.role === "owner" && me.role !== "owner"} className={`flex min-h-14 items-center justify-between gap-3 px-1 text-start ${u.active ? "" : "opacity-50"}`}>
              <span className="font-medium">{u.name}</span>
              <span className="muted">{t(`role_${u.role}`)} · {LANG_NAMES[u.language]}{u.active ? "" : ` · ${t("inactive")}`}</span>
            </button>
          ))}
        </div>
      </Card>
      {edit && (
        <Modal onClose={() => setEdit(null)}>
          <h2 className="mb-3 text-lg font-semibold">{isNew ? t("new_user") : edit.name}</h2>
          <div className="flex flex-col gap-3">
            <Field label={t("name")}>
              <TextInput value={edit.name ?? ""} onChange={(e) => setEdit({ ...edit, name: e.target.value })} autoFocus />
            </Field>
            <Field label={t("role")}>
              <Select value={edit.role ?? "cashier"} onChange={(r) => setEdit({ ...edit, role: r })} options={roleOptions} />
            </Field>
            <Field label={t("pin")} hint={isNew ? undefined : t("pin_keep")}>
              <TextInput type="password" inputMode="numeric" maxLength={6} value={edit.pin ?? ""} onChange={(e) => setEdit({ ...edit, pin: e.target.value.replace(/\D/g, "") })} />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label={t("language")}>
                <Select<Lang> value={edit.language ?? "fr"} onChange={(l) => setEdit({ ...edit, language: l })} options={langOptions} />
              </Field>
              <Field label={t("theme")}>
                <Select value={edit.theme ?? "dark"} onChange={(x) => setEdit({ ...edit, theme: x })} options={[{ value: "dark" as const, label: t("theme_dark") }, { value: "light" as const, label: t("theme_light") }]} />
              </Field>
            </div>
            <Toggle checked={edit.active ?? true} onChange={(v) => setEdit({ ...edit, active: v })} label={t("active")} />
            <div className="mt-2 flex justify-end gap-3">
              <Button onClick={() => setEdit(null)}>{t("cancel")}</Button>
              <Button variant="primary" onClick={save} disabled={!edit.name?.trim() || !pinOk}>{t("save")}</Button>
            </div>
          </div>
        </Modal>
      )}
      {toast.node}
    </div>
  );
}

// ---------------------------------------------------------------- Permissions

export function PermissionsScreen({ me }: { me: SessionUser }) {
  const t = useT();
  const toast = useToast();
  const [data, setData] = useState<{ permissions: string[]; roles: Record<string, string[]> } | null>(null);
  const load = useCallback(() => rpc<typeof data>("permissions.get").then(setData), []);
  useEffect(() => void load(), [load]);
  if (!data) return <p className="muted p-6">{t("loading")}</p>;
  const editable = me.role === "owner";

  const flip = async (role: string, perm: string) => {
    const cur = data.roles[role];
    const next = cur.includes(perm) ? cur.filter((p) => p !== perm) : [...cur, perm];
    try {
      await rpc("permissions.set", { role, permissions: next });
      load();
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  const roles = ["owner", "manager", "cashier", "kitchen"];
  return (
    <div className="mx-auto max-w-4xl p-4">
      <Card title={t("nav_permissions")}>
        <p className="muted mb-3 text-sm">{t("permissions_hint")}</p>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr>
                <th />
                {roles.map((r) => (
                  <th key={r} className="px-2 py-2 text-center font-semibold">{t(`role_${r}` as Key)}</th>
                ))}
              </tr>
            </thead>
            <tbody>
              {data.permissions.map((p) => (
                <tr key={p} className="border-t line">
                  <td className="py-1 pe-3">{t(`perm_${p}` as Key)}</td>
                  {roles.map((r) => {
                    const on = data.roles[r].includes(p);
                    const locked = r === "owner" || !editable;
                    return (
                      <td key={r} className="text-center">
                        <button
                          disabled={locked}
                          onClick={() => flip(r, p)}
                          aria-pressed={on}
                          className={`m-1 h-11 w-11 rounded-lg border line text-lg ${on ? "bg-brand text-white" : "panel-2"} disabled:opacity-60`}
                        >
                          {on ? "✓" : ""}
                        </button>
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </Card>
      {toast.node}
    </div>
  );
}

// ---------------------------------------------------------------- Audit

interface AuditEntry {
  id: number;
  at: string;
  user_name: string | null;
  action: string;
  entity: string | null;
  entity_id: string | null;
  details: unknown;
}

/** Action groups for the filter (prefixes of the logged action names). */
const AUDIT_GROUPS: { id: string; label: Key; prefixes: string[] }[] = [
  { id: "orders", label: "audit_g_orders", prefixes: ["order.", "receipt.", "auth.override"] },
  { id: "cash", label: "audit_g_cash", prefixes: ["cash.", "drawer."] },
  { id: "stock", label: "audit_g_stock", prefixes: ["stock.", "ingredient.", "recipe.", "purchase.", "supplier."] },
  { id: "menu", label: "audit_g_menu", prefixes: ["product.", "category.", "modifier_group.", "station."] },
  { id: "staff", label: "audit_g_staff", prefixes: ["auth.", "user.", "permissions."] },
  { id: "setup", label: "audit_g_setup", prefixes: ["settings.", "setup.", "printer.", "print.", "device.", "floor.", "table."] },
];

export function AuditScreen() {
  const t = useT();
  const toast = useToast();
  const [rows, setRows] = useState<AuditEntry[]>([]);
  const [done, setDone] = useState(false);
  const [users, setUsers] = useState<{ id: number; name: string }[]>([]);
  const [f, setF] = useState({ from: "", to: "", user: 0, group: "", q: "" });
  useEffect(() => void rpc<{ id: number; name: string }[]>("auth.users").then(setUsers, () => {}), []);
  const filter = useCallback(() => {
    const a = parseDay(f.from);
    const b = parseDay(f.to);
    return {
      from: a ? a.toISOString() : undefined,
      to: b ? addDays(b, 1).toISOString() : undefined,
      user_id: f.user || undefined,
      prefixes: AUDIT_GROUPS.find((g) => g.id === f.group)?.prefixes ?? [],
      q: f.q.trim() || undefined,
    };
  }, [f]);
  const more = useCallback(
    (offset: number) =>
      rpc<AuditEntry[]>("audit.list", { ...filter(), limit: 100, offset }).then(
        (r) => {
          setRows((old) => (offset === 0 ? r : [...old, ...r]));
          setDone(r.length < 100);
        },
        (e) => toast.error(errorText(t.lang, e)),
      ),
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [filter],
  );
  useEffect(() => {
    const id = setTimeout(() => void more(0), 250);
    return () => clearTimeout(id);
  }, [more]);
  const fmt = new Intl.DateTimeFormat(t.lang === "ar" ? "ar-MA-u-nu-latn" : t.lang, { dateStyle: "short", timeStyle: "medium" });
  const exportCsv = async () => {
    try {
      const all = await rpc<AuditEntry[]>("audit.list", { ...filter(), limit: 5000, offset: 0 });
      const table: Table = {
        id: "audit", title: t("nav_audit"),
        columns: [{ label: t("audit_when") }, { label: t("audit_who") }, { label: t("audit_action") }, { label: t("audit_details") }],
        rows: all.map((r) => [{ text: fmt.format(new Date(r.at)) }, { text: r.user_name ?? "" }, { text: r.action },
          { text: [r.entity && r.entity_id ? `${r.entity} ${r.entity_id}` : "", r.details ? JSON.stringify(r.details) : ""].filter(Boolean).join(" · ") }]),
      };
      const csv = toCsv([t("nav_audit"), t("rp_generated", { d: fmt.format(new Date()) })], [table], t.lang);
      const day = dayKey(new Date());
      const r = await rpc<{ path: string }>("device.save_export", { name: `${t("audit_file_prefix")}_${day}.csv`, data: toBase64(new TextEncoder().encode(csv)) });
      toast.ok(t("rp_saved", { p: r.path }));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  return (
    <div className="mx-auto max-w-5xl p-4">
      {toast.node}
      <Card title={t("nav_audit")} actions={<Button onClick={exportCsv}><Download className="inline" /> CSV</Button>}>
        <div className="mb-4 grid gap-3 sm:grid-cols-5">
          <Field label={t("rp_from")}>
            <TextInput type="date" value={f.from} onChange={(e) => setF({ ...f, from: e.target.value })} />
          </Field>
          <Field label={t("rp_to")}>
            <TextInput type="date" value={f.to} onChange={(e) => setF({ ...f, to: e.target.value })} />
          </Field>
          <Field label={t("audit_who")}>
            <Select value={f.user} onChange={(v) => setF({ ...f, user: v })} options={[{ value: 0, label: t("audit_everyone") }, ...users.map((u) => ({ value: u.id, label: u.name }))]} />
          </Field>
          <Field label={t("audit_action")}>
            <Select value={f.group} onChange={(v) => setF({ ...f, group: v })} options={[{ value: "", label: t("audit_all_actions") }, ...AUDIT_GROUPS.map((g) => ({ value: g.id, label: t(g.label) }))]} />
          </Field>
          <Field label={t("audit_search")}>
            <TextInput value={f.q} placeholder={t("audit_search_ph")} onChange={(e) => setF({ ...f, q: e.target.value })} />
          </Field>
        </div>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead className="muted text-start">
              <tr>
                <th className="py-2 text-start">{t("audit_when")}</th>
                <th className="text-start">{t("audit_who")}</th>
                <th className="text-start">{t("audit_action")}</th>
                <th className="text-start">{t("audit_details")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr key={r.id} className="border-t line align-top">
                  <td className="py-2 pe-3 whitespace-nowrap">{fmt.format(new Date(r.at))}</td>
                  <td className="pe-3">{r.user_name ?? "—"}</td>
                  <td className="pe-3 font-mono">{r.action}</td>
                  <td className="muted font-mono text-xs break-all" dir="ltr">{r.details ? JSON.stringify(r.details) : ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {rows.length === 0 && <p className="muted py-4">{t("rp_nothing")}</p>}
        </div>
        {!done && <Button className="mt-3 w-full" onClick={() => more(rows.length)}>{t("load_more")}</Button>}
      </Card>
    </div>
  );
}

// ---------------------------------------------------------------- Me

export function MeScreen({ me, onChange }: { me: SessionUser; onChange: (u: SessionUser) => void }) {
  const t = useT();
  const toast = useToast();
  const [cur, setCur] = useState("");
  const [next, setNext] = useState("");

  const setPrefs = async (language: Lang, theme: "dark" | "light") => {
    try {
      onChange(await rpc<SessionUser>("users.set_prefs", { language, theme }));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };
  const changePin = async () => {
    try {
      await rpc("auth.change_pin", { current_pin: cur, new_pin: next });
      setCur("");
      setNext("");
      toast.ok(t("saved"));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  return (
    <div className="mx-auto flex max-w-xl flex-col gap-4 p-4">
      <Card title={me.name}>
        <div className="flex flex-col gap-3">
          <Field label={t("language")}>
            <Select<Lang> value={me.language} onChange={(l) => setPrefs(l, me.theme)} options={langOptions} />
          </Field>
          <Field label={t("theme")}>
            <Select value={me.theme} onChange={(x) => setPrefs(me.language, x)} options={[{ value: "dark" as const, label: t("theme_dark") }, { value: "light" as const, label: t("theme_light") }]} />
          </Field>
        </div>
      </Card>
      <Card title={t("change_pin")}>
        <div className="flex flex-col gap-3">
          <Field label={t("current_pin")}>
            <TextInput type="password" inputMode="numeric" maxLength={6} value={cur} onChange={(e) => setCur(e.target.value.replace(/\D/g, ""))} />
          </Field>
          <Field label={t("new_pin")}>
            <TextInput type="password" inputMode="numeric" maxLength={6} value={next} onChange={(e) => setNext(e.target.value.replace(/\D/g, ""))} />
          </Field>
          <Button variant="primary" disabled={cur.length < 4 || !/^\d{4,6}$/.test(next)} onClick={changePin}>{t("save")}</Button>
        </div>
      </Card>
      {toast.node}
    </div>
  );
}
