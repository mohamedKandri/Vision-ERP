// Floor plan: the board that shows the room (walls, bar, doors…) and the tables, used by the
// POS (pick / open a table) and by the editor (move, resize, rotate anything).
import { useCallback, useEffect, useRef, useState } from "react";
import { useT, type Key } from "../i18n";
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
export interface Box {
  x: number;
  y: number;
  w: number;
  h: number;
  rotation: number;
}
export interface PlanTable extends Box {
  id: number;
  name: string;
  seats: number;
  sort: number;
  active: boolean;
  floor_id: number | null;
  shape: "square" | "round";
  orders: TableOrder[];
}
export type ItemKind = "wall" | "bar" | "door" | "window" | "plant" | "zone" | "label";
export const ITEM_KINDS: ItemKind[] = ["wall", "bar", "door", "window", "plant", "zone", "label"];
export interface FloorItem extends Box {
  id: number;
  floor_id: number;
  kind: ItemKind;
  label: string;
  color: string;
  z: number;
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
  items: FloorItem[];
}

/** Default size and look of each room element (units on the 96 x 64 board). */
export const ITEM_DEFAULTS: Record<ItemKind, { w: number; h: number; color: string }> = {
  wall: { w: 24, h: 1, color: "#374151" },
  bar: { w: 24, h: 5, color: "#8B5E3C" },
  door: { w: 7, h: 2, color: "#D97706" },
  window: { w: 12, h: 1, color: "#38BDF8" },
  plant: { w: 4, h: 4, color: "#16A34A" },
  zone: { w: 24, h: 16, color: "#A855F7" },
  label: { w: 14, h: 4, color: "#6B7280" },
};

/**
 * Loads the plan and keeps it live (orders change table status).
 * `paused()` returning true skips live reloads (the editor, while something is being moved).
 */
export function usePlan(paused?: () => boolean) {
  const [plan, setPlan] = useState<Plan | null>(null);
  const [error, setError] = useState<unknown>(null);
  const pausedRef = useRef(paused);
  pausedRef.current = paused;
  const load = useCallback(async (force?: boolean) => {
    if (force !== true && pausedRef.current?.()) return;
    try {
      setPlan(await rpc<Plan>("tables.plan"));
      setError(null);
    } catch (e) {
      setError(e);
    }
  }, []);
  useEffect(() => {
    load(true);
  }, [load]);
  useLive(["orders", "settings"], () => load(), 20000);
  return { plan, setPlan, error, reload: () => load(true) };
}

export type TableState = "free" | "open" | "ready";
export function tableState(t: PlanTable): TableState {
  if (!t.orders.length) return "free";
  if (t.orders.some((o) => o.status === "ready")) return "ready";
  return "open";
}
const STATE_CLS: Record<TableState, string> = {
  free: "bg-[var(--panel-2)] border-[var(--muted)]",
  open: "bg-blue-600 border-blue-800 text-white",
  ready: "bg-amber-400 border-amber-600 text-stone-900",
};

function minutesSince(iso: string) {
  return Math.max(0, Math.floor((Date.now() - new Date(iso).getTime()) / 60000));
}

/** Selection key: "t:12" (table) or "i:3" (room element). */
export type SelKey = `t:${number}` | `i:${number}`;
type Drag = { key: SelKey; mode: "move" | "resize"; sx: number; sy: number; box: Box; moved: boolean };

/**
 * One floor. In `edit` mode, press and drag moves an element, the corner handle resizes it;
 * `onChange` receives the new box. Otherwise a tap on a table calls `onTap`.
 */
export function FloorBoard({
  plan,
  floorId,
  selectedId,
  edit,
  selected,
  onSelect,
  onChange,
  onTap,
  onDragEnd,
  snap = 1,
}: {
  plan: Plan;
  floorId: number | null;
  selectedId?: number | null;
  edit?: boolean;
  selected?: SelKey | null;
  onSelect?: (k: SelKey | null) => void;
  onChange?: (k: SelKey, b: Box) => void;
  onTap?: (t: PlanTable) => void;
  /** Called when a move / resize ends with the element actually changed. */
  onDragEnd?: (k: SelKey) => void;
  snap?: number;
}) {
  const t = useT();
  const board = useRef<HTMLDivElement>(null);
  const drag = useRef<Drag | null>(null);
  const [, tick] = useState(0);
  useEffect(() => {
    const id = setInterval(() => tick((n) => n + 1), 30000);
    return () => clearInterval(id);
  }, []);
  const { w: GW, h: GH } = plan.grid;
  const tables = plan.tables.filter((x) => x.floor_id === floorId && (edit || x.active));
  const items = plan.items.filter((x) => x.floor_id === floorId);

  const unit = (e: React.PointerEvent) => {
    const r = board.current!.getBoundingClientRect();
    return { ux: ((e.clientX - r.left) / r.width) * GW, uy: ((e.clientY - r.top) / r.height) * GH };
  };
  const startDrag = (e: React.PointerEvent, key: SelKey, box: Box, mode: Drag["mode"]) => {
    if (!edit) return;
    e.stopPropagation();
    board.current?.setPointerCapture?.(e.pointerId);
    const { ux, uy } = unit(e);
    drag.current = { key, mode, sx: ux, sy: uy, box, moved: false };
    onSelect?.(key);
  };
  const r = (v: number) => Math.round(v / snap) * snap;

  const place = (b: Box) => ({
    left: `${(b.x / GW) * 100}%`,
    top: `${(b.y / GH) * 100}%`,
    width: `${(b.w / GW) * 100}%`,
    height: `${(b.h / GH) * 100}%`,
    transform: b.rotation ? `rotate(${b.rotation}deg)` : undefined,
  });

  const handles = (key: SelKey, b: Box) =>
    edit && selected === key ? (
      <>
        <span className="pointer-events-none absolute -inset-1 rounded-[inherit] border-2 border-dashed border-brand" />
        <span
          onPointerDown={(e) => startDrag(e, key, b, "resize")}
          className="absolute -bottom-2 -end-2 h-5 w-5 cursor-nwse-resize rounded-full border-2 border-white bg-brand shadow"
          aria-label={t("fl_resize")}
        />
      </>
    ) : null;

  return (
    <div
      ref={board}
      className="relative w-full touch-none select-none overflow-hidden rounded-md border line"
      style={{
        aspectRatio: `${GW} / ${GH}`,
        backgroundColor: "var(--panel)",
        backgroundImage: edit
          ? "linear-gradient(var(--line) 1px, transparent 1px), linear-gradient(90deg, var(--line) 1px, transparent 1px)"
          : undefined,
        backgroundSize: edit ? `${(4 / GW) * 100}% ${(4 / GH) * 100}%` : undefined,
      }}
      onPointerDown={() => edit && onSelect?.(null)}
      onPointerMove={(e) => {
        const d = drag.current;
        if (!d || !onChange) return;
        const { ux, uy } = unit(e);
        const dx = ux - d.sx;
        const dy = uy - d.sy;
        if (!d.moved && Math.abs(dx) + Math.abs(dy) < 0.6) return;
        d.moved = true;
        const min = d.key.startsWith("t:") ? 2 : 1;
        let b: Box;
        if (d.mode === "move") {
          b = { ...d.box, x: Math.min(Math.max(r(d.box.x + dx), 0), GW - d.box.w), y: Math.min(Math.max(r(d.box.y + dy), 0), GH - d.box.h) };
        } else {
          b = { ...d.box, w: Math.min(Math.max(r(d.box.w + dx), min), GW - d.box.x), h: Math.min(Math.max(r(d.box.h + dy), min), GH - d.box.y) };
        }
        onChange(d.key, b);
      }}
      onPointerUp={() => {
        const d = drag.current;
        drag.current = null;
        if (d?.moved) onDragEnd?.(d.key);
      }}
      onPointerCancel={() => {
        const d = drag.current;
        drag.current = null;
        if (d?.moved) onDragEnd?.(d.key);
      }}
    >
      {items.map((it) => {
        const key: SelKey = `i:${it.id}`;
        const color = it.color || ITEM_DEFAULTS[it.kind].color;
        const look: React.CSSProperties =
          it.kind === "zone"
            ? { background: `${color}22`, border: `2px dashed ${color}` }
            : it.kind === "label"
              ? { color }
              : it.kind === "window"
                ? { background: `${color}88`, border: `1px solid ${color}` }
                : it.kind === "door"
                  ? { background: `${color}33`, borderBottom: `3px solid ${color}` }
                  : { background: color };
        return (
          <div
            key={key}
            onPointerDown={(e) => startDrag(e, key, it, "move")}
            className={`absolute flex items-center justify-center overflow-visible text-center font-semibold ${it.kind === "plant" ? "rounded-full" : "rounded-sm"} ${
              edit ? "cursor-move" : "pointer-events-none"
            } ${it.kind === "bar" || it.kind === "wall" ? "text-white" : ""}`}
            style={{ ...place(it), ...look, zIndex: it.kind === "zone" ? 0 : 1 }}
          >
            {it.kind === "plant" ? <span className="text-[clamp(10px,1.4vw,18px)]">🌿</span> : null}
            {it.kind === "door" && !it.label ? <span className="text-[clamp(8px,1vw,12px)]">🚪</span> : null}
            {it.label && (
              <span className={`truncate px-1 ${it.kind === "label" ? "text-[clamp(11px,1.6vw,20px)]" : "text-[clamp(9px,1.1vw,13px)]"} ${it.kind === "zone" ? "absolute start-1 top-1" : ""}`}
                style={it.kind === "zone" ? { color } : undefined}>
                {it.label}
              </span>
            )}
            {handles(key, it)}
          </div>
        );
      })}
      {tables.map((tb) => {
        const key: SelKey = `t:${tb.id}`;
        const st = tableState(tb);
        const first = tb.orders[0];
        const due = tb.orders.reduce((s, o) => s + o.total_cents - o.paid_cents, 0);
        return (
          <button
            key={key}
            onPointerDown={(e) => startDrag(e, key, tb, "move")}
            onClick={() => {
              if (edit) return;
              onTap?.(tb);
            }}
            className={`absolute z-[2] flex flex-col items-center justify-center border-2 p-0.5 text-center leading-tight shadow-sm ${
              tb.shape === "round" ? "rounded-full" : "rounded-md"
            } ${STATE_CLS[st]} ${selectedId === tb.id ? "ring-4 ring-brand ring-offset-1" : ""} ${tb.active ? "" : "opacity-40"} ${edit ? "cursor-move" : ""}`}
            style={place(tb)}
            aria-label={`${tb.name} · ${t(`fl_state_${st}` as Key)}`}
          >
            <span className="flex flex-col items-center" style={tb.rotation ? { transform: `rotate(${-tb.rotation}deg)` } : undefined}>
              <b className="max-w-full truncate text-[clamp(10px,1.4vw,16px)]">{tb.name}</b>
              {first ? (
                <>
                  <span className="num text-[clamp(9px,1vw,12px)] opacity-90" dir="ltr">{formatMoney(due, t.lang)}</span>
                  <span className="num text-[clamp(8px,0.9vw,11px)] opacity-80">#{first.number} · {minutesSince(first.created_at)}′</span>
                </>
              ) : (
                <span className="text-[clamp(8px,0.9vw,11px)] opacity-70">{tb.seats} 👤</span>
              )}
            </span>
            {handles(key, tb)}
          </button>
        );
      })}
      {!tables.length && !items.length && <p className="muted absolute inset-0 flex items-center justify-center">{t("fl_empty")}</p>}
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
      {(["free", "open", "ready"] as TableState[]).map((s) => (
        <span key={s} className="flex items-center gap-1">
          <span className={`inline-block h-3 w-3 rounded-sm border ${STATE_CLS[s]}`} /> {t(`fl_state_${s}` as Key)}
        </span>
      ))}
    </div>
  );
}
