import QRCode from "qrcode";
import { useCallback, useEffect, useState } from "react";
import { Button, Card, Field, Modal, Select, TextInput, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { setDevice, type DeviceInfo } from "../lib/device";
import { useLive } from "../lib/live";
import { rpc } from "../lib/rpc";

const KINDS = ["pos", "kds", "display", "office"] as const;

interface PairedDevice { id: number; name: string; kind: string; last_seen: string | null; last_ip: string | null; revoked: boolean }

/** Device role: Host (pairing, paired devices) or Client (join / leave). `canManage` = owner/manager on this shop. */
export function DeviceScreen({ canManage, onChanged }: { canManage: boolean; onChanged?: (d: DeviceInfo) => void }) {
  const t = useT();
  const toast = useToast();
  const [dev, setDev] = useState<DeviceInfo | null>(null);
  const [pairing, setPairing] = useState<{ pin: string; addresses: string[]; qr: string; expires_in: number } | null>(null);
  const [qrImg, setQrImg] = useState("");
  const [devices, setDevices] = useState<PairedDevice[]>([]);
  const [joining, setJoining] = useState(false);

  const load = useCallback(async () => {
    try {
      const d = await rpc<DeviceInfo>("device.get");
      setDev(d);
      setDevice(d);
      if (d.mode === "host" && canManage) setDevices(await rpc<PairedDevice[]>("devices.list"));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [canManage]);
  useEffect(() => void load(), [load]);
  useLive(["devices"], load);
  useEffect(() => {
    if (pairing) QRCode.toDataURL(pairing.qr, { margin: 1, width: 220 }).then(setQrImg);
  }, [pairing]);

  if (!dev) return <p className="muted p-6">{t("loading")}</p>;
  const changed = (d: DeviceInfo) => {
    setDev(d);
    setDevice(d);
    onChanged?.(d);
  };
  const save = async (patch: Partial<DeviceInfo>) => {
    try {
      changed(await rpc<DeviceInfo>("device.update", patch));
      toast.ok(t("saved"));
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
  };

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-4 p-4">
      <Card title={t("dev_title")}>
        <div className="flex flex-col gap-3">
          <div className={`rounded-xl p-3 font-semibold ${dev.mode === "host" ? "bg-brand/15" : dev.online ? "bg-emerald-600/15" : "bg-red-600/15"}`}>
            {dev.mode === "host" ? t("dev_mode_host") : `${t("dev_mode_client")} · ${t("dev_connected", { h: `${dev.shop_name} (${dev.host})` })}`}
            {dev.server_error && <div className="text-sm text-red-500">{t("dev_server_error", { e: dev.server_error })}</div>}
          </div>
          <div className="grid gap-3 sm:grid-cols-3">
            <Field label={t("dev_name")}>
              <TextInput defaultValue={dev.name} onBlur={(e) => e.target.value !== dev.name && save({ name: e.target.value })} />
            </Field>
            <Field label={t("dev_kind")}>
              <Select<string> value={dev.kind} onChange={(k) => save({ kind: k as DeviceInfo["kind"] })} options={KINDS.map((k) => ({ value: k, label: t(`dev_kind_${k}` as Key) }))} />
            </Field>
            <Field label={t("dev_register")}>
              <TextInput defaultValue={dev.register} onBlur={(e) => e.target.value.trim() && e.target.value !== dev.register && save({ register: e.target.value.trim() })} />
            </Field>
          </div>
        </div>
      </Card>

      {dev.mode === "host" && canManage && (
        <Card title={t("dev_paired")} actions={<Button variant="primary" onClick={async () => { try { setPairing(await rpc("devices.pairing_start")); } catch (e) { toast.error(errorText(t.lang, e)); } }}>{t("dev_pair")}</Button>}>
          <div className="muted mb-3 text-sm">{t("dev_addresses")}: <b dir="ltr">{dev.addresses.join("  ·  ") || "—"}</b></div>
          <div className="flex flex-col divide-y line">
            {devices.map((d) => (
              <div key={d.id} className={`flex items-center gap-3 py-2 ${d.revoked ? "opacity-40 line-through" : ""}`}>
                <span className="flex-1">
                  <b>{d.name}</b> · {t(`dev_kind_${d.kind}` as Key)}
                  <span className="muted block text-xs">{t("dev_last_seen")}: {d.last_seen ? new Date(d.last_seen).toLocaleString() : "—"} {d.last_ip ? `(${d.last_ip})` : ""}</span>
                </span>
                {!d.revoked && <Button variant="danger" onClick={() => rpc("devices.revoke", { id: d.id }).then(load, (e) => toast.error(errorText(t.lang, e)))}>{t("dev_revoke")}</Button>}
              </div>
            ))}
            {!devices.length && <p className="muted py-3">—</p>}
          </div>
        </Card>
      )}

      {(dev.mode === "client" || canManage) && (
        <Card>
          <div className="flex flex-wrap gap-2">
            {dev.mode === "host" && <Button onClick={() => setJoining(true)}>{t("dev_join")}</Button>}
            {dev.mode === "client" && (
              <Button variant="danger" onClick={async () => {
                if (!confirm(t("dev_leave_confirm"))) return;
                try { changed(await rpc<DeviceInfo>("device.leave")); location.reload(); } catch (e) { toast.error(errorText(t.lang, e)); }
              }}>{t("dev_leave")}</Button>
            )}
          </div>
        </Card>
      )}

      {pairing && (
        <Modal onClose={() => setPairing(null)}>
          <h2 className="mb-2 text-xl font-bold">{t("dev_pair")}</h2>
          <p className="muted mb-4 text-sm">{t("dev_pair_hint")}</p>
          <div className="flex flex-col items-center gap-3">
            {qrImg && <img src={qrImg} alt="QR" className="h-48 w-48 rounded-lg bg-white p-2" />}
            <div className="text-5xl font-bold tracking-[0.3em]" dir="ltr">{pairing.pin}</div>
            <div className="text-lg" dir="ltr">{pairing.addresses.join("  ·  ")}</div>
          </div>
        </Modal>
      )}
      {joining && <JoinDialog onClose={() => setJoining(false)} onJoined={(d) => { changed(d); location.reload(); }} />}
      {toast.node}
    </div>
  );
}

/** Pair this device with a Host. Also used from the first-run screen. */
export function JoinDialog({ onClose, onJoined }: { onClose: () => void; onJoined: (d: DeviceInfo) => void }) {
  const t = useT();
  const [host, setHost] = useState("");
  const [pin, setPin] = useState("");
  const [name, setName] = useState("");
  const [kind, setKind] = useState<DeviceInfo["kind"]>("pos");
  const [register, setRegister] = useState("");
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // Accept a pasted pairing link (the QR content) as well as a plain address.
  const paste = (v: string) => {
    const m = v.match(/host=([^&]+)&code=([^&]+)&pin=(\d{6})/);
    if (m) {
      setHost(decodeURIComponent(m[1]));
      setPin(m[3]);
    } else setHost(v);
  };
  return (
    <Modal onClose={onClose}>
      <h2 className="mb-3 text-xl font-bold">{t("dev_join")}</h2>
      <div className="flex flex-col gap-3">
        <Field label={t("dev_host_address")}><TextInput dir="ltr" inputMode="url" autoFocus value={host} onChange={(e) => paste(e.target.value)} placeholder="192.168.1.20" /></Field>
        <Field label={t("dev_pairing_code")}><TextInput dir="ltr" inputMode="numeric" maxLength={6} value={pin} onChange={(e) => setPin(e.target.value.replace(/\D/g, ""))} /></Field>
        <Field label={t("dev_name")}><TextInput value={name} onChange={(e) => setName(e.target.value)} placeholder="Caisse 2, Cuisine…" /></Field>
        <div className="grid grid-cols-2 gap-3">
          <Field label={t("dev_kind")}><Select<string> value={kind} onChange={(k) => setKind(k as DeviceInfo["kind"])} options={KINDS.map((k) => ({ value: k, label: t(`dev_kind_${k}` as Key) }))} /></Field>
          <Field label={t("dev_register")}><TextInput value={register} onChange={(e) => setRegister(e.target.value)} placeholder="caisse2" /></Field>
        </div>
        {err && <p className="text-red-500">{err}</p>}
        <div className="flex justify-end gap-3">
          <Button onClick={onClose}>{t("cancel")}</Button>
          <Button variant="primary" disabled={busy || !host.trim() || pin.length !== 6 || !name.trim()}
            onClick={async () => {
              setBusy(true);
              setErr(null);
              try {
                onJoined(await rpc<DeviceInfo>("device.join", { host, pin, name, kind, register }));
              } catch (e) {
                setErr(errorText(t.lang, e));
              } finally {
                setBusy(false);
              }
            }}>{t("dev_join")}</Button>
        </div>
      </div>
    </Modal>
  );
}
