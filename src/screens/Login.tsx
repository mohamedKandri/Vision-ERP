import { ArrowLeft, ArrowRight } from "lucide-react";
import { useEffect, useState } from "react";
import { PinPad } from "../components/PinPad";
import { Button } from "../components/ui";
import { errorText, translate } from "../i18n";
import { rpc, setToken, type Lang, type SessionUser } from "../lib/rpc";

interface Tile {
  id: number;
  name: string;
  role: SessionUser["role"];
}

const roleColor: Record<Tile["role"], string> = {
  owner: "bg-amber-500",
  manager: "bg-sky-600",
  cashier: "bg-emerald-600",
  kitchen: "bg-rose-600",
};

export function Login({ shopName, lang, onLogin }: { shopName: string; lang: Lang; onLogin: (u: SessionUser) => void }) {
  const [users, setUsers] = useState<Tile[]>([]);
  const [picked, setPicked] = useState<Tile | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [lockedFor, setLockedFor] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const t = (k: Parameters<typeof translate>[1], v?: Record<string, number>) => translate(lang, k, v);

  useEffect(() => {
    rpc<Tile[]>("auth.users").then(setUsers, (e) => setErr(errorText(lang, e)));
  }, [lang]);

  useEffect(() => {
    if (lockedFor <= 0) return;
    const id = setTimeout(() => setLockedFor((s) => s - 1), 1000);
    return () => clearTimeout(id);
  }, [lockedFor]);

  const login = async (pin: string) => {
    if (!picked) return;
    setBusy(true);
    try {
      const r = await rpc<{ token: string; user: SessionUser }>("auth.login", { user_id: picked.id, pin });
      setToken(r.token);
      onLogin(r.user);
    } catch (e) {
      const ae = e as { code?: string; retry_after_secs?: number };
      if (ae.code === "locked") setLockedFor(ae.retry_after_secs ?? 60);
      setErr(errorText(lang, e));
      setAttempt((a) => a + 1); // remount the pad so it clears
    } finally {
      setBusy(false);
    }
  };

  const Back = lang === "ar" ? ArrowRight : ArrowLeft;

  return (
    <div className="flex min-h-full flex-col items-center justify-center gap-6 p-4">
      <h1 className="text-3xl font-bold">🍔 {shopName}</h1>
      {!picked ? (
        <>
          <p className="muted text-lg">{t("who_are_you")}</p>
          <div className="grid w-full max-w-3xl grid-cols-2 gap-3 sm:grid-cols-3 md:grid-cols-4">
            {users.map((u) => (
              <button
                key={u.id}
                onClick={() => {
                  setPicked(u);
                  setErr(null);
                  setLockedFor(0);
                }}
                className="panel flex min-h-28 flex-col items-center justify-center gap-2 rounded-2xl p-3 active:scale-95 transition"
              >
                <span className={`flex h-12 w-12 items-center justify-center rounded-full text-xl font-bold text-white ${roleColor[u.role]}`}>
                  {u.name.slice(0, 1).toUpperCase()}
                </span>
                <span className="font-semibold">{u.name}</span>
                <span className="muted text-xs">{t(`role_${u.role}`)}</span>
              </button>
            ))}
          </div>
        </>
      ) : (
        <div className="panel w-full max-w-sm rounded-2xl p-5">
          <div className="mb-3 flex items-center gap-3">
            <Button variant="plain" onClick={() => setPicked(null)} aria-label={t("back")}>
              <Back />
            </Button>
            <div>
              <div className="text-lg font-semibold">{picked.name}</div>
              <div className="muted text-sm">{t("enter_pin")}</div>
            </div>
          </div>
          <PinPad key={attempt} onSubmit={login} busy={busy} disabled={lockedFor > 0} error={lockedFor > 0 ? t("locked", { s: lockedFor }) : err} />
        </div>
      )}
    </div>
  );
}
