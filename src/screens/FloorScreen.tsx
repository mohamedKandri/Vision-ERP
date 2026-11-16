// Floor plan editor: draw the shop as it really is — walls, bar, doors, windows, plants, zones,
// labels — and place, resize and rotate every table freely.
import { Copy, Plus, RotateCcw, RotateCw, Save, Trash2 } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import {
  FloorBoard, FloorTabs, ITEM_DEFAULTS, ITEM_KINDS, usePlan,
  type Box, type Floor, type FloorItem, type ItemKind, type PlanTable, type SelKey,
} from "../components/FloorPlan";
import { Button, Field, Modal, Select, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT, type Key } from "../i18n";
import { rpc } from "../lib/rpc";

type TableEdit = Omit<PlanTable, "id" | "orders"> & { id: number | null };
const ITEM_ICON: Record<ItemKind, string> = { wall: "▬", bar: "🍸", door: "🚪", window: "🪟", plant: "🌿", zone: "▢", label: "T" };
const COLORS = ["#374151", "#8B5E3C", "#D97706", "#38BDF8", "#16A34A", "#A855F7", "#DC2626", "#6B7280", "#0F766E", "#F5F5F4"];

/** `onDone`: shown inside the Caisse, with a button back to service. */
export default function FloorScreen({ onDone }: { onDone?: () => void } = {}) {
  const t = useT();
  const toast = useToast();
  // Live reloads would put moved tables back where they were: skip them while edits are unsaved.
  const busy = useRef(false);
  const { plan, setPlan, error, reload } = usePlan(() => busy.current);
  const [floorId, setFloorId] = useState<number | null>(null);
  const [sel, setSel] = useState<SelKey | null>(null);
  const [tableEdit, setTableEdit] = useState<TableEdit | null>(null);
  const [moved, setMoved] = useState<Set<SelKey>>(new Set());
  const [floorEdit, setFloorEdit] = useState<Partial<Floor> | null>(null);
  const [snap, setSnap] = useState(true);
  const fail = (e: unknown) => toast.error(errorText(t.lang, e));

  useEffect(() => {
    if (plan && (floorId === null || !plan.floors.some((f) => f.id === floorId))) setFloorId(plan.floors[0]?.id ?? null);
  }, [plan, floorId]);

  // Auto-save: whatever was moved, resized or rotated is saved shortly after the last change.
  const latest = useRef<(only?: SelKey[]) => Promise<void>>(async () => {});
  useEffect(() => {
    if (!moved.size) return;
    const id = setTimeout(() => latest.current([...moved]), 700);
    return () => clearTimeout(id);
  }, [moved, plan]);

  // Keyboard: arrows nudge, Delete removes, Ctrl+D duplicates.
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (!sel || !plan || (e.target as HTMLElement).closest("input,textarea,select")) return;
      const b = boxOf(sel);
      if (!b) return;
      const step = e.shiftKey ? 4 : 1;
      const d: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] };
      if (d[e.key]) {
        e.preventDefault();
        change(sel, { ...b, x: clamp(b.x + d[e.key][0], 0, plan.grid.w - b.w), y: clamp(b.y + d[e.key][1], 0, plan.grid.h - b.h) });
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        remove(sel);
      } else if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "d") {
        e.preventDefault();
        duplicate(sel);
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });

  if (error && !plan) return <p className="p-6 text-red-500">{errorText(t.lang, error)}</p>;
  if (!plan) return <p className="muted p-6">…</p>;
  const GW = plan.grid.w;
  const GH = plan.grid.h;
  const floor = plan.floors.find((f) => f.id === floorId) ?? null;
  const dirty = moved.size > 0;
  busy.current = dirty;

  function clamp(v: number, a: number, b: number) {
    return Math.min(Math.max(v, a), b);
  }
  function idOf(k: SelKey) {
    return Number(k.slice(2));
  }
  function tableOf(k: SelKey | null) {
    return k?.startsWith("t:") ? plan!.tables.find((x) => x.id === idOf(k)) ?? null : null;
  }
  function itemOf(k: SelKey | null) {
    return k?.startsWith("i:") ? plan!.items.find((x) => x.id === idOf(k)) ?? null : null;
  }
  function boxOf(k: SelKey): Box | null {
    return tableOf(k) ?? itemOf(k);
  }
  function change(k: SelKey, b: Box) {
    const fit = { ...b, x: clamp(b.x, 0, GW - b.w), y: clamp(b.y, 0, GH - b.h) };
    setPlan((p) =>
      p && {
        ...p,
        tables: k.startsWith("t:") ? p.tables.map((x) => (x.id === idOf(k) ? { ...x, ...fit } : x)) : p.tables,
        items: k.startsWith("i:") ? p.items.map((x) => (x.id === idOf(k) ? { ...x, ...fit } : x)) : p.items,
      },
    );
    setMoved((m) => new Set(m).add(k));
  }

  /** First free spot for a w x h box on this floor (scan in steps of 2). */
  const freeSpot = (w: number, h: number) => {
    const on: Box[] = [...plan.tables.filter((x) => x.floor_id === floorId), ...plan.items.filter((x) => x.floor_id === floorId && x.kind !== "zone")];
    for (let y = 2; y + h <= GH; y += 2)
      for (let x = 2; x + w <= GW; x += 2)
        if (on.every((o) => x + w <= o.x || o.x + o.w <= x || y + h <= o.y || o.y + o.h <= y)) return { x, y };
    return { x: 0, y: 0 };
  };

  /** Saves positions. With `only`, saves just those elements quietly (after a drag or a rotation). */
  const saveLayout = async (only?: SelKey[]) => {
    if (!floorId) return;
    const keys = new Set(only ?? moved);
    const pick = ({ id, x, y, w, h, rotation }: Box & { id: number }) => ({ id, x, y, w, h, rotation });
    try {
      await rpc("tables.layout", {
        floor_id: floorId,
        tables: plan.tables.filter((x) => keys.has(`t:${x.id}`) && x.floor_id === floorId).map(pick),
        items: plan.items.filter((x) => keys.has(`i:${x.id}`) && x.floor_id === floorId).map(pick),
      });
      setMoved((m) => {
        const n = new Set(m);
        keys.forEach((k) => n.delete(k));
        return n;
      });
      if (!only) toast.ok(t("saved"));
    } catch (e) {
      fail(e);
    }
  };
  latest.current = saveLayout;

  const addItem = async (kind: ItemKind, from?: FloorItem) => {
    if (!floorId) return;
    const d = ITEM_DEFAULTS[kind];
    const w = from?.w ?? d.w;
    const h = from?.h ?? d.h;
    const spot = from ? { x: clamp(from.x + 3, 0, GW - w), y: clamp(from.y + 3, 0, GH - h) } : freeSpot(w, h);
    try {
      if (dirty) await saveLayout();
      const r = await rpc<{ id: number }>("tables.save_item", {
        id: null, floor_id: floorId, kind, label: from?.label ?? (kind === "label" ? t("fl_kind_label") : kind === "zone" ? t("fl_kind_zone") : ""),
        color: from?.color ?? "", w, h, rotation: from?.rotation ?? 0, z: kind === "zone" ? -1 : 0, ...spot,
      });
      await reload();
      setSel(`i:${r.id}`);
    } catch (e) {
      fail(e);
    }
  };

  const newTable = (shape: "square" | "round", w: number, h: number, seats: number, from?: PlanTable) => {
    if (!floor) return;
    const spot = from ? { x: clamp(from.x + 4, 0, GW - w), y: clamp(from.y + 4, 0, GH - h) } : freeSpot(w, h);
    const n = plan.tables.length + 1;
    setTableEdit({ id: null, name: from ? `${from.name} bis` : `T${n}`, seats, sort: n, active: true, floor_id: floor.id, w, h, shape, rotation: from?.rotation ?? 0, ...spot });
  };

  const saveTable = async () => {
    if (!tableEdit) return;
    try {
      if (dirty) await saveLayout();
      const r = await rpc<{ id: number }>("tables.save", tableEdit);
      setTableEdit(null);
      await reload();
      setSel(`t:${r.id}`);
      toast.ok(t("saved"));
    } catch (e) {
      fail(e);
    }
  };

  const remove = async (k: SelKey) => {
    const tb = tableOf(k);
    const it = itemOf(k);
    if (!confirm(t("fl_delete_q", { name: tb?.name ?? it?.label ?? t(`fl_kind_${it?.kind}` as Key) }))) return;
    try {
      if (tb) await rpc("tables.delete", { id: tb.id });
      else if (it) await rpc("tables.delete_item", { id: it.id });
      setSel(null);
      setMoved((m) => {
        const n = new Set(m);
        n.delete(k);
        return n;
      });
      reload();
    } catch (e) {
      fail(e);
    }
  };

  const duplicate = (k: SelKey) => {
    const tb = tableOf(k);
    const it = itemOf(k);
    if (tb) newTable(tb.shape, tb.w, tb.h, tb.seats, tb);
    else if (it) addItem(it.kind, it);
  };

  const saveItemProps = async (it: FloorItem, patch: Partial<FloorItem>) => {
    const next = { ...it, ...patch };
    setPlan((p) => p && { ...p, items: p.items.map((x) => (x.id === it.id ? next : x)) });
    try {
      await rpc("tables.save_item", next);
    } catch (e) {
      fail(e);
      reload();
    }
  };

  const saveFloor = async () => {
    if (!floorEdit) return;
    try {
      const r = await rpc<{ id: number }>("tables.save_floor", { id: floorEdit.id ?? null, name: floorEdit.name ?? "", sort: floorEdit.sort ?? plan.floors.length + 1, active: floorEdit.active ?? true });
      setFloorEdit(null);
      setFloorId(r.id);
      reload();
    } catch (e) {
      fail(e);
    }
  };
  const deleteFloor = async () => {
    if (!floorEdit?.id) return;
    try {
      await rpc("tables.delete_floor", { id: floorEdit.id });
      setFloorEdit(null);
      setFloorId(null);
      reload();
    } catch (e) {
      fail(e);
    }
  };

  const selTable = tableOf(sel);
  const selItem = itemOf(sel);
  const selBox = selTable ?? selItem;
  const rotate = (deg: number) => sel && selBox && change(sel, { ...selBox, rotation: (((selBox.rotation + deg) % 360) + 360) % 360 });
  const tool = "flex h-10 items-center gap-1.5 rounded-sm border line panel-2 px-3 text-[13px] font-semibold hover:border-brand";

  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* toolbar */}
      <div className="flex flex-wrap items-center gap-2 border-b line bg-[var(--panel)] px-3 py-2">
        <FloorTabs floors={plan.floors} value={floorId} all onChange={(id) => {
          if (dirty) saveLayout();
          setMoved(new Set());
          setSel(null);
          setFloorId(id);
        }} />
        <button className={tool} onClick={() => setFloorEdit({ name: "" })}><Plus className="h-4 w-4" />{t("fl_add_floor")}</button>
        {floor && <button className={tool} onClick={() => setFloorEdit(floor)}>{t("fl_edit_floor")}</button>}
        <div className="flex-1" />
        <Toggle checked={snap} onChange={setSnap} label={t("fl_snap")} />
        <span className="muted text-xs">{dirty ? t("fl_saving") : t("fl_autosaved")}</span>
        <Button variant="primary" disabled={!dirty} onClick={() => saveLayout()}><Save className="h-4 w-4" /> {t("fl_save_layout")}</Button>
        {onDone && <Button onClick={async () => { if (dirty) await saveLayout(); onDone(); }}>{t("fl_done")}</Button>}
      </div>

      {floor ? (
        <div className="grid min-h-0 flex-1 grid-cols-1 gap-px bg-[var(--line)] lg:grid-cols-[180px_minmax(0,1fr)_260px]">
          {/* palette */}
          <aside className="flex flex-col gap-1.5 overflow-auto bg-[var(--panel)] p-2">
            <p className="label px-1">{t("fl_tables")}</p>
            <button className={tool} onClick={() => newTable("round", 8, 8, 2)}>⬤ {t("fl_tpl_round2")}</button>
            <button className={tool} onClick={() => newTable("round", 12, 12, 4)}>⬤ {t("fl_tpl_round4")}</button>
            <button className={tool} onClick={() => newTable("square", 10, 10, 4)}>■ {t("fl_tpl_square4")}</button>
            <button className={tool} onClick={() => newTable("square", 20, 10, 6)}>▬ {t("fl_tpl_rect6")}</button>
            <button className={tool} onClick={() => newTable("square", 28, 10, 8)}>▬ {t("fl_tpl_rect8")}</button>
            <p className="label mt-2 px-1">{t("fl_room")}</p>
            {ITEM_KINDS.map((k) => (
              <button key={k} className={tool} onClick={() => addItem(k)}>
                <span className="w-5 text-center">{ITEM_ICON[k]}</span>{t(`fl_kind_${k}` as Key)}
              </button>
            ))}
          </aside>

          {/* board */}
          <div className="min-h-0 overflow-auto bg-[var(--bg)] p-3">
            <FloorBoard plan={plan} floorId={floorId} edit selected={sel} onSelect={setSel} onChange={change} snap={snap ? 2 : 1} />
            <p className="muted mt-2 text-xs">{t("fl_editor_hint")}</p>
          </div>

          {/* inspector */}
          <aside className="flex flex-col gap-3 overflow-auto bg-[var(--panel)] p-3">
            {!selBox ? (
              <p className="muted text-sm">{t("fl_select_hint")}</p>
            ) : (
              <>
                <div className="flex items-center justify-between gap-2">
                  <b className="truncate text-base">{selTable ? selTable.name : t(`fl_kind_${selItem!.kind}` as Key)}</b>
                  {selTable && <span className="muted text-xs">{selTable.seats} 👤</span>}
                </div>
                {selTable && <Button onClick={() => setTableEdit({ ...selTable })}>{t("fl_table_details")}</Button>}
                {selItem && (
                  <>
                    <Field label={t("fl_label")}>
                      <TextInput value={selItem.label} maxLength={40} onChange={(e) => setPlan((p) => p && { ...p, items: p.items.map((x) => (x.id === selItem.id ? { ...x, label: e.target.value } : x)) })}
                        onBlur={(e) => saveItemProps(selItem, { label: e.target.value })} />
                    </Field>
                    <div>
                      <p className="label mb-1">{t("fl_color")}</p>
                      <div className="flex flex-wrap gap-1.5">
                        {COLORS.map((c) => (
                          <button key={c} onClick={() => saveItemProps(selItem, { color: c })} aria-label={c}
                            className={`h-7 w-7 rounded-sm border-2 ${(selItem.color || ITEM_DEFAULTS[selItem.kind].color).toLowerCase() === c.toLowerCase() ? "border-brand" : "border-transparent"}`}
                            style={{ background: c }} />
                        ))}
                      </div>
                    </div>
                  </>
                )}
                <div className="grid grid-cols-2 gap-2">
                  <Field label={t("fl_width")}><TextInput type="number" min={1} max={GW} value={selBox.w} onChange={(e) => change(sel!, { ...selBox, w: clamp(Number(e.target.value) || 1, selTable ? 2 : 1, GW - selBox.x) })} /></Field>
                  <Field label={t("fl_height")}><TextInput type="number" min={1} max={GH} value={selBox.h} onChange={(e) => change(sel!, { ...selBox, h: clamp(Number(e.target.value) || 1, selTable ? 2 : 1, GH - selBox.y) })} /></Field>
                </div>
                <div>
                  <p className="label mb-1">{t("fl_rotation")} · <span className="num">{selBox.rotation}°</span></p>
                  <input type="range" min={0} max={359} step={snap ? 15 : 1} value={selBox.rotation} className="w-full accent-[var(--color-brand)]"
                    onChange={(e) => change(sel!, { ...selBox, rotation: Number(e.target.value) })} />
                  <div className="mt-1 grid grid-cols-3 gap-1">
                    <button className={tool} onClick={() => rotate(-15)}><RotateCcw className="h-4 w-4" />15°</button>
                    <button className={tool} onClick={() => rotate(15)}><RotateCw className="h-4 w-4" />15°</button>
                    <button className={tool} onClick={() => rotate(90)}><RotateCw className="h-4 w-4" />90°</button>
                  </div>
                </div>
                <div className="grid grid-cols-2 gap-2">
                  <button className={tool} onClick={() => duplicate(sel!)}><Copy className="h-4 w-4" />{t("fl_duplicate")}</button>
                  <button className={`${tool} text-red-600`} onClick={() => remove(sel!)}><Trash2 className="h-4 w-4" />{t("fl_delete")}</button>
                </div>
              </>
            )}
          </aside>
        </div>
      ) : (
        <p className="muted py-10 text-center">{t("fl_no_floor")}</p>
      )}

      {tableEdit && (
        <Modal onClose={() => setTableEdit(null)}>
          <h2 className="mb-4 text-xl font-bold">{tableEdit.id ? tableEdit.name : t("fl_add_table")}</h2>
          <div className="grid grid-cols-2 gap-3">
            <Field label={t("name")}><TextInput autoFocus value={tableEdit.name} onChange={(e) => setTableEdit({ ...tableEdit, name: e.target.value })} /></Field>
            <Field label={t("fl_seats")}><TextInput type="number" min={1} max={50} value={tableEdit.seats} onChange={(e) => setTableEdit({ ...tableEdit, seats: Number(e.target.value) || 1 })} /></Field>
            <Field label={t("fl_shape")}>
              <Select<string> value={tableEdit.shape} onChange={(v) => setTableEdit({ ...tableEdit, shape: v as "square" | "round" })}
                options={[{ value: "square", label: t("fl_shape_square") }, { value: "round", label: t("fl_shape_round") }]} />
            </Field>
            <Field label={t("fl_floor")}>
              <Select<number> value={tableEdit.floor_id ?? 0} onChange={(v) => setTableEdit({ ...tableEdit, floor_id: v })} options={plan.floors.map((f) => ({ value: f.id, label: f.name }))} />
            </Field>
          </div>
          <div className="mt-3"><Toggle checked={tableEdit.active} onChange={(v) => setTableEdit({ ...tableEdit, active: v })} label={t("active")} /></div>
          <div className="mt-5 flex justify-end gap-2">
            <Button onClick={() => setTableEdit(null)}>{t("cancel")}</Button>
            <Button variant="primary" onClick={saveTable}>{t("save")}</Button>
          </div>
        </Modal>
      )}

      {floorEdit && (
        <Modal onClose={() => setFloorEdit(null)}>
          <h2 className="mb-4 text-xl font-bold">{floorEdit.id ? t("fl_edit_floor") : t("fl_add_floor")}</h2>
          <Field label={t("name")}><TextInput autoFocus placeholder="Salle, Terrasse, Étage…" value={floorEdit.name ?? ""} onChange={(e) => setFloorEdit({ ...floorEdit, name: e.target.value })} /></Field>
          {floorEdit.id && <div className="mt-3"><Toggle checked={floorEdit.active ?? true} onChange={(v) => setFloorEdit({ ...floorEdit, active: v })} label={t("active")} /></div>}
          <div className="mt-5 flex justify-between gap-2">
            {floorEdit.id ? <Button onClick={deleteFloor}><Trash2 className="h-4 w-4" /> {t("fl_delete_floor")}</Button> : <span />}
            <div className="flex gap-2">
              <Button onClick={() => setFloorEdit(null)}>{t("cancel")}</Button>
              <Button variant="primary" onClick={saveFloor}>{t("save")}</Button>
            </div>
          </div>
        </Modal>
      )}
      {toast.node}
    </div>
  );
}
