import { Check, Undo2 } from "lucide-react";
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

/** Kitchen Display: live tickets as cards, tap items, bump tickets, recall. */
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
  // Every open ticket, all stations: the tabs count per station and the view filters here.
  const [all, setAll] = useState<Ticket[]>([]);
  const [limits, setLimits] = useState({ warn: 5, late: 10 });
  const [now, setNow] = useState(Date.now());
  const seen = useRef<Set<number>>(new Set());
  const loaded = useRef(false);

  useEffect(() => {
    rpc<Station[]>("kds.stations").then(setStations, () => {});
    rpc<Settings>("settings.get").then((s) => setLimits({ warn: Number(s.kds_warn_minutes) || 5, late: Number(s.kds_late_minutes) || 10 }), () => {});
    const id = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(id);
  }, []);

  const load = useCallback(async () => {
    try {
      const b = await rpc<Ticket[]>("kds.board", { station_id: null });
      loaded.current = true;
      setAll(b);
    } catch (e) {
      toast.error(errorText(t.lang, e));
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  useEffect(() => void load(), [load]);
  useLive(["orders"], load, 5000);

  // A ticket as this screen shows it: only the chosen station's items.
  const forStation = (tk: Ticket, sid: number | null): Ticket | null => {
    const items = sid === null ? tk.items : tk.items.filter((i) => i.station_id === sid);
    return items.some((i) => !i.voided) ? { ...tk, items } : null;
  };
  const tickets = all.map((tk) => forStation(tk, station)).filter((x): x is Ticket => !!x);
  const count = (sid: number | null) => all.filter((tk) => forStation(tk, sid)).length;

  // Beep when a new ticket appears for this station. Not on the first load, not on switching station.
  const seenFor = useRef<number | null | undefined>(undefined);
  useEffect(() => {
    if (!loaded.current) return;
    const ids = tickets.map((tk) => tk.order_id);
    if (seenFor.current === station && ids.some((id) => !seen.current.has(id))) beep();
    if (seenFor.current !== station) seen.current = new Set();
    ids.forEach((id) => seen.current.add(id));
    seenFor.current = station;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [all, station]);

  const pick = (id: number | null) => {
    setStation(id);
    try {
      localStorage.setItem("burger.kds.station", id ? String(id) : "");
    } catch {
      /* ignore */
    }
  };
  const act = (p: Promise<unknown>) => p.then(load, (e) => toast.error(errorText(t.lang, e)));

  const ageOf = (tk: Ticket) => Math.max(0, now - new Date(tk.sent_at).getTime());
  const stateOf = (tk: Ticket): "ok" | "soon" | "late" => {
    const mins = ageOf(tk) / 60000;
    return mins >= limits.late ? "late" : mins >= limits.warn ? "soon" : "ok";
  };
  const states = tickets.map(stateOf);
  const n = (st: string) => states.filter((x) => x === st).length;

  // "To prepare": everything not done yet on the tickets shown, added up by product.
  const todo = new Map<string, number>();
  for (const tk of tickets) for (const it of tk.items) {
    if (it.done || it.voided) continue;
    const name = pickName(it.names, t.lang);
    todo.set(name, (todo.get(name) ?? 0) + it.qty);
  }
  const todoList = [...todo.entries()].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));

  const head = {
    ok: "bg-[var(--sel)] text-[#0c3b5e] dark:text-[#cfe4fa]",
    soon: "bg-[#f7d064] text-[#3b2a00]",
    late: "bg-[#c50f1f] text-white",
  };

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 flex-wrap items-end gap-x-4 gap-y-2 border-b line bg-[var(--panel)] px-4">
        <div role="tablist" aria-label={t("kds_stations")} className="flex gap-0.5 overflow-x-auto">
          {[{ id: null as number | null, label: t("kds_all") }, ...stations.map((s) => ({ id: s.id as number | null, label: pickName(s.names, t.lang) }))].map((s) => (
            <button key={s.id ?? "all"} role="tab" aria-selected={station === s.id} onClick={() => pick(s.id)}
              className={`min-h-[52px] shrink-0 px-4 text-base ${station === s.id ? "font-semibold shadow-[inset_0_-3px_0_var(--color-brand)]" : "muted hov"}`}>
              {s.label} <span className="muted font-normal">({count(s.id)})</span>
            </button>
          ))}
        </div>
        <div className="ms-auto flex flex-wrap items-center gap-2 pb-2">
          {n("ok") > 0 && <span className="rounded-full bg-[#f1faf1] px-3 py-1 font-semibold text-[#0e700e] dark:bg-emerald-500/15 dark:text-emerald-300">{t("kds_n_on_time", { n: n("ok") })}</span>}
          {n("soon") > 0 && <span className="rounded-full bg-[#fff4ce] px-3 py-1 font-semibold text-[#835b00] dark:bg-amber-500/15 dark:text-amber-300">{t("kds_n_soon", { n: n("soon") })}</span>}
          {n("late") > 0 && <span className="rounded-full bg-[#fdf3f4] px-3 py-1 font-semibold text-[#b10e1c] dark:bg-red-500/15 dark:text-red-300">{t("kds_n_late", { n: n("late") })}</span>}
          {station !== null && (
            <Button onClick={() => act(rpc("kds.recall", { station_id: station }))}><Undo2 className="me-1 inline h-4 w-4 text-brand" />{t("kds_recall")}</Button>
          )}
        </div>
      </div>

      <div className="flex min-h-0 flex-1 flex-wrap content-start items-start gap-4 overflow-auto p-4">
        <div className="grid min-w-0 flex-[999_1_640px] grid-cols-[repeat(auto-fill,minmax(260px,1fr))] items-start gap-3.5">
          {tickets.map((tk, idx) => {
            const st = states[idx];
            const age = ageOf(tk);
            const mins = Math.floor(age / 60000);
            const secs = Math.floor(age / 1000) % 60;
            const live = tk.items.filter((i) => !i.voided);
            const done = live.filter((i) => i.done).length;
            const ticketStations = station !== null ? [station] : [...new Set(live.map((i) => i.station_id))];
            return (
              <article key={tk.order_id} data-testid="kds-ticket"
                className={`card flex flex-col overflow-hidden rounded-2xl ${st === "late" ? "ring-2 ring-[#c50f1f]" : ""}`}>
                <div className={`flex items-center justify-between gap-2 px-3.5 py-2.5 ${head[st]}`}>
                  <span className="flex min-w-0 flex-col">
                    <b className="num text-3xl leading-none">#{tk.split_index ? `${tk.number}-${tk.split_index + 1}` : tk.number}</b>
                    <span className="mt-1 truncate text-[13px] font-semibold uppercase">
                      {t(`order_type_${tk.type}` as Key)}{tk.table_name ? ` · ${tk.table_name}` : ""}{tk.customer_name ? ` · ${tk.customer_name}` : ""}
                    </span>
                  </span>
                  <span className="flex shrink-0 flex-col items-end">
                    <b className="num text-2xl" dir="ltr">{String(mins).padStart(2, "0")}:{String(secs).padStart(2, "0")}</b>
                    <span className={`text-xs font-semibold ${st === "late" ? "uppercase" : ""}`}>{t(st === "ok" ? "kds_on_time" : st === "soon" ? "kds_soon" : "kds_late")}</span>
                  </span>
                </div>
                <ul className="py-1">
                  {tk.items.map((it) => {
                    const details = it.details[t.lang] ?? it.details.fr ?? [];
                    return (
                      <li key={it.id} className="border-b line last:border-b-0">
                        <button disabled={it.voided} aria-pressed={it.done} onClick={() => act(rpc("kds.toggle", { id: it.id }))}
                          className={`hov flex min-h-[52px] w-full gap-3 px-3.5 py-2.5 text-start ${it.done ? "muted" : ""} ${it.voided ? "text-red-500 opacity-60" : ""}`}>
                          <span aria-hidden="true" className={`mt-0.5 flex h-[26px] w-[26px] shrink-0 items-center justify-center rounded-md border-2 ${it.done ? "border-[#0e700e] bg-[#0e700e] text-white" : "border-[var(--input-bottom)]"}`}>
                            {it.done && <Check className="h-4 w-4" strokeWidth={3} />}
                          </span>
                          <span className={`flex min-w-0 flex-col gap-0.5 ${it.done || it.voided ? "line-through" : ""}`}>
                            <b className="text-lg">{it.qty} × {pickName(it.names, t.lang)}</b>
                            {details.length > 0 && <span className="muted text-[15px] font-semibold">{details.join(" · ")}</span>}
                            {it.note && <span className="mt-0.5 self-start rounded bg-yellow-300 px-2 py-0.5 text-sm font-bold text-stone-900">⚠ {it.note}</span>}
                          </span>
                        </button>
                      </li>
                    );
                  })}
                </ul>
                {tk.note && <div className="px-3.5 pb-1 text-sm font-semibold text-amber-700 dark:text-amber-400">{t("pos_note")}: {tk.note}</div>}
                <div className="mt-auto flex flex-col gap-1.5 px-3.5 pb-3.5 pt-2">
                  {ticketStations.map((sid) => (
                    <Button key={sid} variant="primary" className="h-14 text-lg" onClick={() => act(rpc("kds.bump", { order_id: tk.order_id, station_id: sid }))}>
                      {t("kds_bump")}
                      {station === null && ticketStations.length > 1 ? ` · ${pickName(stations.find((s) => s.id === sid)?.names, t.lang)}` : ` · ${done}/${live.length}`}
                    </Button>
                  ))}
                </div>
              </article>
            );
          })}
          {!tickets.length && <p className="muted col-span-full py-20 text-center text-2xl">{t("kds_empty")}</p>}
        </div>

        {todoList.length > 0 && (
          <aside aria-label={t("kds_todo")} className="card min-w-0 flex-[1_1_240px] rounded-2xl lg:max-w-[300px]" data-testid="kds-todo">
            <h2 className="px-4 pt-3.5 text-lg font-semibold">{t("kds_todo")}</h2>
            <p className="muted px-4 pb-2 text-[13px]">{t("kds_todo_hint")}</p>
            {todoList.map(([name, qty]) => (
              <div key={name} className="flex items-baseline justify-between gap-2 border-t line px-4 py-2.5">
                <span className="text-base font-semibold">{name}</span>
                <b className="num text-2xl">{qty}</b>
              </div>
            ))}
          </aside>
        )}
      </div>
      {toast.node}
    </div>
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
