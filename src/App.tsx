import { Ban, Banknote, ChefHat, History, Lock, LogOut, Percent, Settings as Cog, ShieldCheck, ShoppingCart, User, Users, UtensilsCrossed } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { applyLang, errorText, LangContext, translate, useT, type Key } from "./i18n";
import { rpc, setToken, type Lang, type SessionUser, type Settings } from "./lib/rpc";
import { AuditScreen, MeScreen, PermissionsScreen, TaxesScreen, UsersScreen } from "./screens/AdminScreens";
import { CashScreen } from "./screens/CashScreen";
import { Login } from "./screens/Login";
import { PosScreen } from "./screens/PosScreen";
import { AvailabilityScreen, MenuScreen } from "./screens/MenuScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import { Setup } from "./screens/Setup";

interface Status {
  setup_done: boolean;
  shop_name: string;
  default_language: Lang;
  version: string;
}

type Page = "pos" | "cash" | "kitchen" | "menu" | "86" | "settings" | "taxes" | "users" | "permissions" | "audit" | "me";

const NAV: { page: Page; label: Key; icon: ReactNode; perm?: string | string[]; phase?: number }[] = [
  { page: "pos", label: "nav_pos", icon: <ShoppingCart />, perm: "pos.sell" },
  { page: "cash", label: "nav_cash", icon: <Banknote />, perm: ["cash.session", "cash.in_out"] },
  { page: "kitchen", label: "nav_kitchen", icon: <ChefHat />, perm: "kds.use", phase: 5 },
  { page: "86", label: "nav_86", icon: <Ban />, perm: ["menu.manage", "pos.sell", "kds.use"] },
  { page: "menu", label: "nav_menu", icon: <UtensilsCrossed />, perm: "menu.manage" },
  { page: "settings", label: "nav_settings", icon: <Cog />, perm: "settings.manage" },
  { page: "taxes", label: "nav_taxes", icon: <Percent />, perm: "settings.manage" },
  { page: "users", label: "nav_users", icon: <Users />, perm: "users.manage" },
  { page: "permissions", label: "nav_permissions", icon: <ShieldCheck />, perm: "users.manage" },
  { page: "audit", label: "nav_audit", icon: <History />, perm: "audit.view" },
  { page: "me", label: "nav_me", icon: <User /> },
];

function applyTheme(theme: "dark" | "light") {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [user, setUser] = useState<SessionUser | null>(null);

  const loadStatus = useCallback(() => {
    rpc<Status>("app.status").then(setStatus, (e) => setFatal(errorText("fr", e)));
  }, []);
  useEffect(loadStatus, [loadStatus]);

  const lang: Lang = user?.language ?? status?.default_language ?? "fr";
  useEffect(() => applyLang(lang), [lang]);
  useEffect(() => applyTheme(user?.theme ?? "dark"), [user?.theme]);

  const logout = useCallback(async () => {
    try {
      await rpc("auth.logout");
    } catch {
      /* session may already be gone */
    }
    setToken(null);
    setUser(null);
  }, []);

  if (fatal) return <p className="p-6 text-red-500">{fatal}</p>;
  if (!status) return <p className="muted p-6">{translate(lang, "loading")}</p>;

  return (
    <LangContext.Provider value={lang}>
      {!status.setup_done ? (
        <Setup onDone={loadStatus} />
      ) : !user ? (
        <Login shopName={status.shop_name} lang={lang} onLogin={setUser} />
      ) : (
        <Shell
          user={user}
          setUser={setUser}
          onLogout={logout}
          onSettings={(s) => setStatus({ ...status, shop_name: String(s.shop_name), default_language: s.default_language as Lang })}
          version={status.version}
        />
      )}
    </LangContext.Provider>
  );
}

function Shell({
  user,
  setUser,
  onLogout,
  onSettings,
  version,
}: {
  user: SessionUser;
  setUser: (u: SessionUser) => void;
  onLogout: () => void;
  onSettings: (s: Settings) => void;
  version: string;
}) {
  const t = useT();
  const can = (p?: string | string[]) => !p || (Array.isArray(p) ? p : [p]).some((x) => user.permissions.includes(x));
  const items = NAV.filter((n) => can(n.perm));
  const [page, setPage] = useState<Page>(items[0]?.page ?? "me");
  useAutoLock(onLogout);

  const current = NAV.find((n) => n.page === page)!;
  let body: ReactNode;
  if (current.phase) {
    body = (
      <div className="flex h-full flex-col items-center justify-center gap-3 p-6 text-center">
        <div className="text-6xl">🍔</div>
        <p className="muted text-lg">{t("coming_phase", { n: current.phase })}</p>
      </div>
    );
  } else {
    switch (page) {
      case "pos":
        body = <PosScreen me={user} />;
        break;
      case "cash":
        body = <CashScreen me={user} />;
        break;
      case "menu":
        body = <MenuScreen />;
        break;
      case "86":
        body = <AvailabilityScreen />;
        break;
      case "settings":
        body = <SettingsScreen canEdit={can("settings.manage")} onSaved={onSettings} />;
        break;
      case "taxes":
        body = <TaxesScreen canEdit={can("settings.manage")} />;
        break;
      case "users":
        body = <UsersScreen me={user} />;
        break;
      case "permissions":
        body = <PermissionsScreen me={user} />;
        break;
      case "audit":
        body = <AuditScreen />;
        break;
      default:
        body = <MeScreen me={user} onChange={setUser} />;
    }
  }

  return (
    <div className="flex h-full flex-col md:flex-row">
      <nav className="panel order-last flex shrink-0 overflow-x-auto border-t md:order-first md:w-56 md:flex-col md:border-e md:border-t-0">
        <div className="hidden p-4 md:block">
          <div className="text-lg font-bold">🍔 BURGER OS</div>
          <div className="muted text-sm">
            {user.name} · {t(`role_${user.role}`)}
          </div>
        </div>
        {items.map((n) => (
          <button
            key={n.page}
            onClick={() => setPage(n.page)}
            className={`flex min-h-14 min-w-20 shrink-0 flex-col items-center justify-center gap-1 px-3 text-xs md:flex-row md:justify-start md:gap-3 md:text-base ${
              page === n.page ? "bg-brand/20 text-brand font-semibold" : ""
            }`}
          >
            {n.icon}
            <span>{t(n.label)}</span>
          </button>
        ))}
        <div className="md:mt-auto" />
        <button onClick={onLogout} className="flex min-h-14 min-w-20 shrink-0 flex-col items-center justify-center gap-1 px-3 text-xs md:flex-row md:justify-start md:gap-3 md:text-base">
          <Lock />
          <span>{t("lock")}</span>
        </button>
        <div className="muted hidden px-4 pb-3 text-xs md:flex md:items-center md:gap-2">
          <LogOut className="h-3 w-3" /> v{version}
        </div>
      </nav>
      <main className="min-h-0 flex-1 overflow-auto">{body}</main>
    </div>
  );
}

/** Lock the screen after `auto_lock_minutes` of no touches (0 = never). */
function useAutoLock(onLock: () => void) {
  const [minutes, setMinutes] = useState(0);
  useEffect(() => {
    rpc<Settings>("settings.get").then((s) => setMinutes(Number(s.auto_lock_minutes) || 0), () => {});
  }, []);
  useEffect(() => {
    if (!minutes) return;
    let id = setTimeout(onLock, minutes * 60_000);
    const reset = () => {
      clearTimeout(id);
      id = setTimeout(onLock, minutes * 60_000);
    };
    const events = ["pointerdown", "keydown"] as const;
    events.forEach((e) => window.addEventListener(e, reset));
    return () => {
      clearTimeout(id);
      events.forEach((e) => window.removeEventListener(e, reset));
    };
  }, [minutes, onLock]);
}
