// Floor plan app: floors (rooms) and tables. Drag tables to arrange them, tap one to edit it.
import { Plus, Save, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { FloorBoard, FloorTabs, StateLegend, usePlan, type Floor, type PlanTable } from "../components/FloorPlan";
import { Button, Card, Field, Modal, Select, TextInput, Toggle, useToast } from "../components/ui";
import { errorText, useT } from "../i18n";
import { rpc } from "../lib/rpc";

type TableEdit = Omit<PlanTable, "id" | "orders"> & { id: number | null };

export default function FloorScreen() {
  const t = useT();
  const toast = useToast();
  const { plan, setPlan, error, reload } = usePlan();
  const [floorId, setFloorId] = useState<number | null>(null);
  const [sel, setSel] = useState<TableEdit | null>(null);
  const [moved, setMoved] = useState<Set<number>>(new Set());
  const [floorEdit, setFloorEdit] = useState<Partial<Floor> | null>(null);
  const fail = (e: unknown) => toast.error(errorText(t.lang, e));

  useEffect(() => {
    if (plan && (floorId === null || !plan.floors.some((f) => f.id === floorId))) setFloorId(plan.floors[0]?.id ?? null);
  }, [plan, floorId]);

  if (error && !plan) return <p className="p-6 text-red-500">{errorText(t.lang, error)}</p>;
  if (!plan) return <p className="muted p-6">…</p>;
  const floor = plan.floors.find((f) => f.id === floorId) ?? null;
  const dirty = moved.size > 0;

  const saveLayout = async () => {
    if (!floorId) return;
    try {
      const tables = plan.tables.filter((x) => moved.has(x.id)).map(({ id, x, y, w, h }) => ({ id, x, y, w, h }));
      await rpc("tables.layout", { floor_id: floorId, tables });
      setMoved(new Set());
      toast.ok(t("saved"));
      reload();
    } catch (e) {
      fail(e);
    }
  };

  /** A free spot for a new table (first place where a 3x3 fits). */
  const freeSpot = () => {
    const on = plan.tables.filter((x) => x.floor_id === floorId);
    for (let y = 0; y + 3 <= plan.grid.h; y++)
      for (let x = 0; x + 3 <= plan.grid.w; x++)
        if (on.every((o) => x + 3 <= o.x || o.x + o.w <= x || y + 3 <= o.y || o.y + o.h <= y)) return { x, y };
    return { x: 0, y: 0 };
  };

  const saveTable = async () => {
    if (!sel) return;
    try {
      await rpc("tables.save", sel);
      setSel(null);
      setMoved((m) => {
        const n = new Set(m);
        if (sel.id) n.delete(sel.id);
        return n;
      });
      toast.ok(t("saved"));
      reload();
    } catch (e) {
      fail(e);
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

  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-4 p-4">
      <Card
        title={t("nav_floor")}
        actions={
          <div className="flex flex-wrap gap-2">
            <Button onClick={() => setFloorEdit({ name: "" })}><Plus className="h-4 w-4" /> {t("fl_add_floor")}</Button>
            {floor && <Button onClick={() => setFloorEdit(floor)}>{t("fl_edit_floor")}</Button>}
            {floor && (
              <Button variant="primary" onClick={() => setSel({ id: null, name: "", seats: 4, sort: plan.tables.length + 1, active: true, floor_id: floor.id, w: 3, h: 3, shape: "square", ...freeSpot() })}>
                <Plus className="h-4 w-4" /> {t("fl_add_table")}
              </Button>
            )}
          </div>
        }
      >
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3">
          <FloorTabs floors={plan.floors} value={floorId} all onChange={(id) => {
            if (dirty && !confirm(t("fl_unsaved"))) return;
            setMoved(new Set());
            if (dirty) reload();
            setFloorId(id);
          }} />
          <StateLegend />
        </div>
        {floor ? (
          <FloorBoard
            plan={plan}
            floorId={floorId}
            edit
            selectedId={sel?.id}
            onTap={(tb) => setSel({ ...tb })}
            onMove={(id, x, y) => {
              setPlan({ ...plan, tables: plan.tables.map((tb) => (tb.id === id ? { ...tb, x, y } : tb)) });
              setMoved((m) => new Set(m).add(id));
            }}
          />
        ) : (
          <p className="muted py-10 text-center">{t("fl_no_floor")}</p>
        )}
        <div className="mt-3 flex items-center justify-between gap-3">
          <p className="muted text-sm">{t("fl_drag_hint")}</p>
          <Button variant="primary" disabled={!dirty} onClick={saveLayout}><Save className="h-4 w-4" /> {t("fl_save_layout")}</Button>
        </div>
      </Card>

      {sel && (
        <Modal onClose={() => setSel(null)}>
          <h2 className="mb-4 text-xl font-bold">{sel.id ? sel.name : t("fl_add_table")}</h2>
          <div className="grid grid-cols-2 gap-3">
            <Field label={t("name")}><TextInput autoFocus value={sel.name} onChange={(e) => setSel({ ...sel, name: e.target.value })} /></Field>
            <Field label={t("fl_seats")}><TextInput type="number" min={1} max={50} value={sel.seats} onChange={(e) => setSel({ ...sel, seats: Number(e.target.value) || 1 })} /></Field>
            <Field label={t("fl_shape")}>
              <Select<string> value={sel.shape} onChange={(v) => setSel({ ...sel, shape: v as "square" | "round" })}
                options={[{ value: "square", label: t("fl_shape_square") }, { value: "round", label: t("fl_shape_round") }]} />
            </Field>
            <Field label={t("fl_floor")}>
              <Select<number> value={sel.floor_id ?? 0} onChange={(v) => setSel({ ...sel, floor_id: v })} options={plan.floors.map((f) => ({ value: f.id, label: f.name }))} />
            </Field>
            <Field label={t("fl_width")}>
              <Select<number> value={sel.w} onChange={(v) => setSel({ ...sel, w: v, x: Math.min(sel.x, plan.grid.w - v) })} options={[1, 2, 3, 4, 5, 6, 8].map((n) => ({ value: n, label: String(n) }))} />
            </Field>
            <Field label={t("fl_height")}>
              <Select<number> value={sel.h} onChange={(v) => setSel({ ...sel, h: v, y: Math.min(sel.y, plan.grid.h - v) })} options={[1, 2, 3, 4, 5, 6, 8].map((n) => ({ value: n, label: String(n) }))} />
            </Field>
          </div>
          <div className="mt-3"><Toggle checked={sel.active} onChange={(v) => setSel({ ...sel, active: v })} label={t("active")} /></div>
          <div className="mt-5 flex justify-end gap-2">
            <Button onClick={() => setSel(null)}>{t("cancel")}</Button>
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
