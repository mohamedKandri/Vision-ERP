import { useEffect, useState } from "react";
import { PinPad } from "../components/PinPad";
import { errorText, translate } from "../i18n";
import { rpc, setToken, type Lang, type SessionUser } from "../lib/rpc";

interface Tile {
  id: number;
  name: string;
  role: SessionUser["role"];
}

const roleColor: Record<Tile["role"], string> = {
  owner: "bg-slate-600",
  manager: "bg-blue-700",
  cashier: "bg-emerald-700",
  kitchen: "bg-amber-700",
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

  return (
    <div className="flex h-full flex-col">
      <header className="bar flex h-9 shrink-0 items-center gap-5 px-3 text-[13px]">
        <b className="num text-white">BURGER/OS</b>
        <span>{shopName}</span>
        <div className="flex-1" />
        <span className="num">{new Date().toLocaleDateString()}</span>
      </header>
      <div className="flex flex-1 items-center justify-center p-4">
        <div className="flex w-full max-w-4xl flex-col gap-px border line bg-[var(--line)] md:flex-row">
          <section className="flex-1 bg-[var(--panel)] p-6">
            <h1 className="m-0 text-xl font-bold">{t("who_are_you")}</h1>
            <p className="muted mb-4 mt-1 text-sm">{t("enter_pin")}</p>
            <div className="flex flex-col">
              {users.map((u) => (
                <button
                  key={u.id}
                  onClick={() => {
                    setPicked(u);
                    setErr(null);
                    setLockedFor(0);
                  }}
                  className={`flex h-14 items-center gap-3 border-b line px-2 text-start ${picked?.id === u.id ? "sel" : "hover:bg-[var(--panel-2)]"}`}
                >
                  <span className={`flex h-9 w-9 items-center justify-center rounded-sm text-sm font-bold text-white ${roleColor[u.role]}`}>{u.name.slice(0, 2).toUpperCase()}</span>
                  <span className="flex-1 font-semibold">{u.name}</span>
                  <span className="label">{t(`role_${u.role}`)}</span>
                </button>
              ))}
            </div>
          </section>
          <section className="w-full bg-[var(--panel)] p-6 md:w-[380px]">
            {picked ? (
              <>
                <div className="label mb-1">{t("pin")}</div>
                <div className="mb-4 text-lg font-bold">{picked.name}</div>
                <PinPad key={attempt} onSubmit={login} busy={busy} disabled={lockedFor > 0} error={lockedFor > 0 ? t("locked", { s: lockedFor }) : err} />
              </>
            ) : (
              <div className="muted flex h-full min-h-60 items-center justify-center text-center">{t("who_are_you")}</div>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
