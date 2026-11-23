import { useEffect, useState, type ButtonHTMLAttributes, type ReactNode } from "react";

type Variant = "primary" | "ghost" | "danger" | "plain";

const variants: Record<Variant, string> = {
  primary: "bg-brand text-white hover:bg-brand-strong active:brightness-90 font-semibold border border-transparent",
  ghost: "bg-[var(--panel)] border line-strong hov font-semibold",
  danger: "bg-[#c50f1f] text-white hover:bg-[#a10e1b] active:brightness-90 font-semibold border border-transparent",
  plain: "hov",
};

/** Min 48 px tall: usable with greasy fingers. */
export function Button({
  variant = "ghost",
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant }) {
  return (
    <button
      {...rest}
      className={`min-h-12 rounded-md px-4 text-base transition disabled:opacity-40 disabled:pointer-events-none ${variants[variant]} ${className}`}
    />
  );
}

export function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-sm font-medium muted">{label}</span>
      {children}
      {hint && <span className="text-xs muted">{hint}</span>}
    </label>
  );
}

/** Input: white box, darker bottom edge, blue underline when focused. */
export const inputCls =
  "min-h-12 w-full rounded-md border line-strong border-b-[var(--input-bottom)] bg-[var(--panel)] px-3 text-base outline-none focus:border-b-brand focus:shadow-[inset_0_-1px_0_var(--color-brand)]";

export function TextInput(props: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input {...props} className={`${inputCls} ${props.className ?? ""}`} />;
}

export function Select<T extends string | number>({
  value,
  onChange,
  options,
}: {
  value: T;
  onChange: (v: T) => void;
  options: { value: T; label: string }[];
}) {
  return (
    <select
      className={inputCls}
      value={String(value)}
      onChange={(e) => {
        const o = options.find((o) => String(o.value) === e.target.value);
        if (o) onChange(o.value);
      }}
    >
      {options.map((o) => (
        <option key={String(o.value)} value={String(o.value)}>
          {o.label}
        </option>
      ))}
    </select>
  );
}

export function Toggle({ checked, onChange, label }: { checked: boolean; onChange: (v: boolean) => void; label: string }) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      onClick={() => onChange(!checked)}
      className="flex min-h-12 items-center justify-between gap-4 rounded-md border line-strong bg-[var(--panel)] px-3 text-start"
    >
      <span>{label}</span>
      <span className={`relative h-6 w-11 shrink-0 rounded-full border transition ${checked ? "border-transparent bg-brand" : "border-[var(--input-bottom)] bg-transparent"}`}>
        <span
          className={`absolute top-[3px] h-4 w-4 rounded-full transition-all ${checked ? "start-[22px] bg-white" : "start-[3px] bg-[var(--input-bottom)]"}`}
        />
      </span>
    </button>
  );
}

export function Card({ title, children, actions }: { title?: string; children: ReactNode; actions?: ReactNode }) {
  return (
    <section className="card rounded-md p-4 sm:p-5">
      {(title || actions) && (
        <div className="mb-4 flex flex-wrap items-center justify-between gap-3">
          {title && <h2 className="text-xl font-semibold">{title}</h2>}
          {actions}
        </div>
      )}
      {children}
    </section>
  );
}

export function Modal({ children, onClose, wide }: { children: ReactNode; onClose: () => void; wide?: boolean }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 p-4" onClick={onClose}>
      <div className={`card max-h-[92vh] w-full ${wide ? "max-w-4xl" : "max-w-lg"} overflow-auto rounded-3xl p-6 shadow-2xl`} onClick={(e) => e.stopPropagation()}>
        {children}
      </div>
    </div>
  );
}

/** Small transient message at the bottom of the screen. */
export function useToast() {
  const [msg, setMsg] = useState<{ text: string; error: boolean } | null>(null);
  useEffect(() => {
    if (!msg) return;
    const id = setTimeout(() => setMsg(null), msg.error ? 5000 : 2000);
    return () => clearTimeout(id);
  }, [msg]);
  const node = msg ? (
    <div
      role="status"
      className={`fixed bottom-4 start-1/2 z-[60] -translate-x-1/2 rtl:translate-x-1/2 rounded-md px-5 py-3 font-medium shadow-lg ${
        msg.error ? "bg-[#c50f1f] text-white" : "bg-[#0e700e] text-white"
      }`}
    >
      {msg.text}
    </div>
  ) : null;
  return {
    node,
    ok: (text: string) => setMsg({ text, error: false }),
    error: (text: string) => setMsg({ text, error: true }),
  };
}

/** Money input in DH; stores integer cents. `nullable` lets the field be emptied (returns null). */
export function MoneyField({
  cents,
  onChange,
  nullable = false,
  placeholder,
}: {
  cents: number | null;
  onChange: (c: number | null) => void;
  nullable?: boolean;
  placeholder?: string;
}) {
  const fmt = (c: number | null) => (c === null ? "" : (c / 100).toFixed(2));
  const [text, setText] = useState(fmt(cents));
  useEffect(() => setText(fmt(cents)), [cents]);
  const commit = () => {
    if (nullable && text.trim() === "") return onChange(null);
    const v = parseMoneyLoose(text);
    if (v === null || v < 0) setText(fmt(cents));
    else onChange(v);
  };
  return (
    <TextInput
      inputMode="decimal"
      value={text}
      placeholder={placeholder}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()}
      dir="ltr"
    />
  );
}

function parseMoneyLoose(s: string): number | null {
  const t = s.trim().replace(/\s/g, "").replace(",", ".");
  if (!/^\d+(\.\d{0,2})?$/.test(t)) return null;
  const [i, f = ""] = t.split(".");
  return parseInt(i, 10) * 100 + parseInt((f + "00").slice(0, 2), 10);
}

/** Resize an uploaded image to a small data URL so the DB and the network stay light. */
export async function shrinkImage(file: File, max: number, type: "image/png" | "image/jpeg" = "image/jpeg"): Promise<string> {
  const bmp = await createImageBitmap(file);
  const scale = Math.min(1, max / Math.max(bmp.width, bmp.height));
  const c = document.createElement("canvas");
  c.width = Math.round(bmp.width * scale);
  c.height = Math.round(bmp.height * scale);
  const ctx = c.getContext("2d")!;
  if (type === "image/jpeg") {
    ctx.fillStyle = "#fff";
    ctx.fillRect(0, 0, c.width, c.height);
  }
  ctx.drawImage(bmp, 0, 0, c.width, c.height);
  return c.toDataURL(type, 0.82);
}

export function Tabs<T extends string>({ value, onChange, tabs }: { value: T; onChange: (t: T) => void; tabs: { id: T; label: string }[] }) {
  return (
    <div role="tablist" className="flex gap-1 overflow-x-auto border-b line">
      {tabs.map((t) => (
        <button
          key={t.id}
          role="tab"
          aria-selected={value === t.id}
          onClick={() => onChange(t.id)}
          className={`min-h-12 shrink-0 px-4 hov ${value === t.id ? "font-semibold shadow-[inset_0_-3px_0_var(--color-brand)]" : "muted"}`}
        >
          {t.label}
        </button>
      ))}
    </div>
  );
}
