import { Undo2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { useLive } from "../lib/live";
import { pickName, type Names } from "../lib/menu";
import { rpc, type Lang, type Settings } from "../lib/rpc";

interface Item { id: number; station_id: number; qty: number; names: Names; details: Record<Lang, string[]>; note: string; sent_at: string; done: boolean; voided: boolean }
interface Ticket { order_id: number; number: number; split_index: number; type: string; table_name: string | null; customer_name: string; note: string; sent_at: string; items: Item[] }
interface Station { id: number; code: string; names: Names }

function beep() {
  try {
    const ac = new AudioContext();
    const o = ac.createOscillator();
    o.frequency.value = 660;
    o.connect(ac.destination);
    o.start();
    o.stop(ac.currentTime + 0.3);
  } catch {
    /* no audio */
  }
}

/** Kitchen Display: live tickets per station, tap items, bump tickets, recall. */
export function KitchenScreen() {
  const t = useT();
  const toast = useToast();
  const [stations, setStations] = useState<Station[]>([]);
  const [station, setStation] = useState<number | null>(() => {
    try {
      return Number(localStorage.getItem("burger.kds.station")) || null;
    } catch {
      return null;
    }
  });
  const [tickets, setTickets] = useState<Ticket[]>([]);
  const [limits, setLimits] = useState({ warn: 5, late: 10 });
  const [now, setNow] = useState(Date.now());
  const seen = useRef<Set<number>>(new Set());

  useEffect(() => {
    rpc<Station[]>("kds.stations").then(setStations, () => {});
    rpc<Settings>("settings.get").then((s) => setLimits({ warn: Number(s.kds_warn_minutes) || 5, late: Number(s.kds_late_minutes) || 10 }), () => {});
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const load = useCallback(async () => {
    try {
      const b = await rpc<Ticket[]>("kds.board", { station_id: station });
      const fresh = b.some((x) => !seen.current.has(x.order_id));
      if (fresh && seen.current.size) beep();
      b.forEach((x) => seen.current.add(x.order_id));
      setTickets(b);
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [station]);
  useEffect(() => void load(), [load]);
  useLive(["orders"], load, 5000);

  const pick = (id: number | null) => {
    setStation(id);
    try {
      localStorage.setItem("burger.kds.station", id ? String(id) : "");
    } catch {
      /* ignore */
    }
  };
  const act = (p: Promise<unknown>) => p.then(load, (e) => toast.error(errorText(t.lang, e)));

  return (
    <div className="flex h-full flex-col">
      <div className="flex flex-wrap items-center gap-2 border-b line p-2">
        <StationBtn active={station === null} onClick={() => pick(null)} label={t("kds_all")} />
        {stations.map((s) => <StationBtn key={s.id} active={station === s.id} onClick={() => pick(s.id)} label={pickName(s.names, t.lang)} />)}
        <div className="ms-auto">
          {station !== null && <Button onClick={() => act(rpc("kds.recall", { station_id: station }))}><Undo2 className="inline h-4 w-4" /> {t("kds_recall")}</Button>}
        </div>
      </div>
      <div className="grid min-h-0 flex-1 auto-rows-min grid-cols-1 gap-3 overflow-auto p-3 sm:grid-cols-2 lg:grid-cols-3 2xl:grid-cols-4">
        {tickets.map((tk) => {
          const mins = Math.floor((now - new Date(tk.sent_at).getTime()) / 60000);
          const secs = Math.floor((now - new Date(tk.sent_at).getTime()) / 1000) % 60;
          const color = mins >= limits.late ? "bg-red-600" : mins >= limits.warn ? "bg-orange-500" : "bg-emerald-600";
          const stationsInTicket = [...new Set(tk.items.map((i) => i.station_id))];
          return (
            <div key={tk.order_id} className="panel flex flex-col overflow-hidden rounded-2xl">
              <div className={`flex items-center justify-between px-3 py-2 text-white ${color}`}>
                <span className="text-3xl font-black">#{tk.split_index ? `${tk.number}-${tk.split_index + 1}` : tk.number}</span>
                <span className="text-right">
                  <span className="block text-sm font-semibold">{t(`order_type_${tk.type}` as Key)}{tk.table_name ? ` · ${tk.table_name}` : ""}{tk.customer_name ? ` · ${tk.customer_name}` : ""}</span>
                  <span className="block font-mono text-lg font-bold" dir="ltr">{String(mins).padStart(2, "0")}:{String(secs).padStart(2, "0")}</span>
                </span>
              </div>
              <div className="flex-1">
                {tk.items.map((it) => (
                  <button key={it.id} disabled={it.voided} onClick={() => act(rpc("kds.toggle", { id: it.id }))}
                    className={`block w-full border-b line px-3 py-2 text-start ${it.done ? "opacity-40 line-through" : ""} ${it.voided ? "bg-red-600/10 line-through opacity-60" : ""}`}>
                    <span className="text-xl font-bold">{it.qty} × {pickName(it.names, t.lang)}</span>
                    {(it.details[t.lang] ?? it.details.fr ?? []).map((d, i) => <span key={i} className="block font-semibold">{d}</span>)}
                    {it.note && <span className="mt-1 block rounded bg-amber-400 px-2 py-0.5 font-bold text-stone-950">⚠ {it.note}</span>}
                  </button>
                ))}
                {tk.note && <div className="bg-amber-400/20 px-3 py-2 font-semibold">📝 {tk.note}</div>}
              </div>
              <div className="flex gap-2 p-2">
                {(station !== null ? [station] : stationsInTicket).map((sid) => (
                  <Button key={sid} variant="primary" className="flex-1 text-lg" onClick={() => act(rpc("kds.bump", { order_id: tk.order_id, station_id: sid }))}>
                    {t("kds_bump")}{station === null ? ` · ${pickName(stations.find((s) => s.id === sid)?.names, t.lang)}` : ""}
                  </Button>
                ))}
              </div>
            </div>
          );
        })}
        {!tickets.length && <p className="muted col-span-full py-20 text-center text-2xl">{t("kds_empty")}</p>}
      </div>
      {toast.node}
    </div>
  );
}

function StationBtn({ active, onClick, label }: { active: boolean; onClick: () => void; label: string }) {
  return (
    <button onClick={onClick} className={`min-h-12 rounded-xl px-4 font-semibold ${active ? "bg-brand text-stone-950" : "panel-2 border line"}`}>
      {label}
    </button>
  );
}

/** Customer-facing "Now preparing / Ready" board. No login needed (shows only order numbers). */
export function CustomerDisplay({ shopName, onExit }: { shopName: string; onExit?: () => void }) {
  const t = useT();
  const [board, setBoard] = useState<{ preparing: string[]; ready: string[] }>({ preparing: [], ready: [] });
  const last = useRef<string[]>([]);
  const load = useCallback(async () => {
    try {
      const b = await rpc<{ preparing: string[]; ready: string[] }>("display.board");
      if (b.ready.some((n) => !last.current.includes(n)) && last.current.length) beep();
      last.current = b.ready;
      setBoard(b);
    } catch {
      /* keep last board */
    }
  }, []);
  useEffect(() => void load(), [load]);
  useLive(["orders"], load, 5000);
  return (
    <div className="flex h-full flex-col bg-stone-950 text-white">
      <div className="flex items-center justify-between px-6 py-4 text-3xl font-bold">
        <span>🍔 {shopName}</span>
        {onExit && <button className="h-12 rounded-xl px-4 text-base opacity-40" onDoubleClick={onExit}>⚙</button>}
      </div>
      <div className="grid min-h-0 flex-1 grid-cols-2 gap-6 p-6">
        <div className="flex flex-col rounded-3xl bg-stone-800 p-6">
          <h2 className="mb-6 text-center text-4xl font-bold text-amber-400">{t("disp_preparing")}</h2>
          <div className="flex flex-wrap content-start justify-center gap-4">
            {board.preparing.map((n) => <span key={n} className="rounded-2xl bg-stone-700 px-6 py-3 text-5xl font-black">{n}</span>)}
          </div>
        </div>
        <div className="flex flex-col rounded-3xl bg-emerald-700 p-6">
          <h2 className="mb-6 text-center text-4xl font-bold">{t("disp_ready")}</h2>
          <div className="flex flex-wrap content-start justify-center gap-4">
            {board.ready.map((n) => <span key={n} className="animate-pulse rounded-2xl bg-white px-6 py-3 text-6xl font-black text-emerald-700">{n}</span>)}
          </div>
        </div>
      </div>
    </div>
  );
}
