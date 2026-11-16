// Floor plan: the board that shows tables on a grid, used by the POS (pick / open a table)
// and by the Floor plan app (drag tables to arrange the room).
import { useCallback, useEffect, useRef, useState } from "react";
import { useT } from "../i18n";
import { useLive } from "../lib/live";
import { formatMoney } from "../lib/money";
import { rpc } from "../lib/rpc";

export interface TableOrder {
  id: number;
  number: number;
  split_index: number;
  status: string;
  payment_status: string;
  total_cents: number;
  paid_cents: number;
  created_at: string;
}
export interface PlanTable {
  id: number;
  name: string;
  seats: number;
  sort: number;
  active: boolean;
  floor_id: number | null;
  x: number;
  y: number;
  w: number;
  h: number;
  shape: "square" | "round";
  orders: TableOrder[];
}
export interface Floor {
  id: number;
  name: string;
  sort: number;
  active: boolean;
}
export interface Plan {
  grid: { w: number; h: number };
  floors: Floor[];
  tables: PlanTable[];
}

/** Loads the plan and keeps it live (orders change table status). */
export function usePlan() {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [error, setError] = useState<unknown>(null);
  const load = useCallback(async () => {
    try {
      setPlan(await rpc<Plan>("tables.plan"));
      setError(null);
    } catch (e) {
      setError(e);
    }
  }, []);
  useEffect(() => {
    load();
  }, [load]);
  useLive(["orders", "settings"], load, 20000);
  return { plan, setPlan, error, reload: load };
}

export type TableState = "free" | "open" | "ready" | "paid";
export function tableState(t: PlanTable): TableState {
  if (!t.orders.length) return "free";
  if (t.orders.every((o) => o.payment_status === "paid")) return "paid";
  if (t.orders.some((o) => o.status === "ready")) return "ready";
  return "open";
}
const STATE_CLS: Record<TableState, string> = {
  free: "panel-2 border-[var(--line)]",
  open: "bg-blue-600 border-blue-700 text-white",
  ready: "bg-amber-400 border-amber-500 text-stone-900",
  paid: "bg-emerald-600 border-emerald-700 text-white",
};

function minutesSince(iso: string) {
  return Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 60000));
}

/**
 * One floor drawn on a grid. In `edit` mode tables can be dragged (snapped to the grid) and
 * `onMove` receives the new position; otherwise a tap calls `onTap`.
 */
export function FloorBoard({
  plan,
  floorId,
  selectedId,
  edit,
  onTap,
  onMove,
}: {
  plan: Plan;
  floorId: number | null;
  selectedId?: number | null;
  edit?: boolean;
  onTap?: (t: PlanTable) => void;
  onMove?: (id: number, x: number, y: number) => void;
}) {
  const t = useT();
  const board = useRef<HTMLDivElement>(null);
  const drag = useRef<{ id: number; dx: number; dy: number; moved: boolean } | null>(null);
  const [, tick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 30000);
    return () => clearInterval(id);
  }, []);
  const { w: GW, h: GH } = plan.grid;
  const tables = plan.tables.filter((x) => x.floor_id === floorId && (edit || x.active));

  const cell = (e: React.PointerEvent) => {
    const r = board.current!.getBoundingClientRect();
    return { cx: ((e.clientX - r.left) / r.width) * GW, cy: ((e.clientY - r.top) / r.height) * GH };
  };

  return (
    <div
      ref={board}
      className="relative w-full touch-none select-none rounded-md border line"
      style={{
        aspectRatio: `${GW} / ${GH}`,
        backgroundImage: edit
          ? "linear-gradient(var(--line) 1px, transparent 1px), linear-gradient(90deg, var(--line) 1px, transparent 1px)"
          : undefined,
        backgroundSize: edit ? `${100 / GW}% ${100 / GH}%` : undefined,
        background: edit ? undefined : "var(--panel)",
      }}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d || !onMove) return;
        const tb = tables.find((x) => x.id === d.id)!;
        const { cx, cy } = cell(e);
        const x = Math.min(Math.max(Math.round(cx - d.dx), 0), GW - tb.w);
        const y = Math.min(Math.max(Math.round(cy - d.dy), 0), GH - tb.h);
        if (x !== tb.x || y !== tb.y) {
          d.moved = true;
          onMove(d.id, x, y);
        }
      }}
      onPointerUp={() => (drag.current = null)}
      onPointerLeave={() => (drag.current = null)}
    >
      {tables.map((tb) => {
        const st = tableState(tb);
        const first = tb.orders[0];
        const total = tb.orders.reduce((s, o) => s + o.total_cents - o.paid_cents, 0);
        return (
          <button
            key={tb.id}
            onPointerDown={(e) => {
              if (!edit) return;
              (e.currentTarget.parentElement as HTMLElement).setPointerCapture?.(e.pointerId);
              const { cx, cy } = cell(e);
              drag.current = { id: tb.id, dx: cx - tb.x, dy: cy - tb.y, moved: false };
            }}
            onClick={() => {
              if (edit && drag.current?.moved) return;
              onTap?.(tb);
            }}
            className={`absolute flex flex-col items-center justify-center overflow-hidden border-2 p-1 text-center leading-tight shadow-sm transition-colors ${
              tb.shape === "round" ? "rounded-full" : "rounded-md"
            } ${STATE_CLS[st]} ${selectedId === tb.id ? "ring-4 ring-brand ring-offset-1" : ""} ${tb.active ? "" : "opacity-40"} ${edit ? "cursor-move" : ""}`}
            style={{
              left: `${(tb.x / GW) * 100}%`,
              top: `${(tb.y / GH) * 100}%`,
              width: `calc(${(tb.w / GW) * 100}% - 6px)`,
              height: `calc(${(tb.h / GH) * 100}% - 6px)`,
              margin: 3,
            }}
            aria-label={`${tb.name} · ${t(`fl_state_${st}`)}`}
          >
            <b className="truncate text-[clamp(11px,1.6vw,16px)]">{tb.name}</b>
            {first ? (
              <>
                <span className="num text-[11px] opacity-90" dir="ltr">{formatMoney(total, t.lang)}</span>
                <span className="num text-[10px] opacity-80">#{first.number} · {minutesSince(first.created_at)}′</span>
              </>
            ) : (
              <span className="text-[11px] opacity-70">{tb.seats} 👤</span>
            )}
          </button>
        );
      })}
      {!tables.length && <p className="muted absolute inset-0 flex items-center justify-center">{t("fl_empty")}</p>}
    </div>
  );
}

/** Floor tabs above the board. */
export function FloorTabs({ floors, value, onChange, all }: { floors: Floor[]; value: number | null; onChange: (id: number) => void; all?: boolean }) {
  return (
    <div className="flex flex-wrap gap-1">
      {floors
        .filter((f) => all || f.active)
        .map((f) => (
          <button key={f.id} onClick={() => onChange(f.id)}
            className={`h-10 rounded-md border px-4 text-sm font-semibold ${value === f.id ? "border-brand bg-brand text-white" : "line panel-2"} ${f.active ? "" : "opacity-50"}`}>
            {f.name}
          </button>
        ))}
    </div>
  );
}

export function StateLegend() {
  const t = useT();
  return (
    <div className="muted flex flex-wrap gap-3 text-xs">
      {(["free", "open", "ready", "paid"] as TableState[]).map((s) => (
        <span key={s} className="flex items-center gap-1">
          <span className={`inline-block h-3 w-3 rounded-sm border ${STATE_CLS[s]}`} /> {t(`fl_state_${s}`)}
        </span>
      ))}
    </div>
  );
}
