import { Delete } from "lucide-react";
import { useEffect, useState } from "react";

/** Big numeric keypad. Calls onSubmit when the user presses OK (or 6 digits are entered). */
export function PinPad({
  onSubmit,
  error,
  busy,
  disabled,
}: {
  onSubmit: (pin: string) => void;
  error?: string | null;
  busy?: boolean;
  disabled?: boolean;
}) {
  const [pin, setPin] = useState("");

  useEffect(() => {
    if (error) setPin("");
  }, [error]);

  const press = (d: string) => {
    if (disabled || busy) return;
    if (pin.length >= 6) return;
    const next = pin + d;
    setPin(next);
    if (next.length === 6) onSubmit(next);
  };
  const submit = () => pin.length >= 4 && !busy && !disabled && onSubmit(pin);

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if (/^\d$/.test(e.key)) press(e.key);
      else if (e.key === "Backspace") setPin((p) => p.slice(0, -1));
      else if (e.key === "Enter") submit();
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });

  const key = "h-16 rounded-2xl panel-2 border line text-2xl font-semibold active:scale-95 transition disabled:opacity-40";
  return (
    <div className="mx-auto flex w-full max-w-xs flex-col gap-4" dir="ltr">
      <div className="flex h-8 items-center justify-center gap-3" aria-label="PIN">
        {Array.from({ length: Math.max(4, pin.length) }, (_, i) => (
          <span key={i} className={`h-4 w-4 rounded-full ${i < pin.length ? "bg-brand" : "border-2 line"}`} />
        ))}
      </div>
      <p className="h-6 text-center text-sm font-medium text-red-500">{error}</p>
      <div className="grid grid-cols-3 gap-3">
        {["1", "2", "3", "4", "5", "6", "7", "8", "9"].map((d) => (
          <button key={d} className={key} onClick={() => press(d)} disabled={disabled}>
            {d}
          </button>
        ))}
        <button className={key} onClick={() => setPin((p) => p.slice(0, -1))} aria-label="delete" disabled={disabled}>
          <Delete className="mx-auto" />
        </button>
        <button className={key} onClick={() => press("0")} disabled={disabled}>
          0
        </button>
        <button
          className={`${key} !bg-brand text-white`}
          onClick={submit}
          disabled={disabled || busy || pin.length < 4}
        >
          OK
        </button>
      </div>
    </div>
  );
}
