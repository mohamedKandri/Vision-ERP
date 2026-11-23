import FloorScreen from "./screens/FloorScreen";
import { BarChart3, Ban, Contact, Banknote, Boxes, Truck, ChefHat, LayoutDashboard, LayoutGrid, Printer, Search, Wifi, History, Lock, Percent, Settings as Cog, ShieldCheck, ShoppingCart, User, Users, UtensilsCrossed } from "lucide-react";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { applyLang, errorText, LangContext, translate, useT, type Key } from "./i18n";
import { rpc, setToken, type Lang, type SessionUser, type Settings } from "./lib/rpc";
import { loadDevice, type DeviceInfo } from "./lib/device";
import { useOnline } from "./lib/live";
import { DeviceScreen, JoinDialog } from "./screens/DeviceScreen";
import { CustomerDisplay, KitchenScreen } from "./screens/KitchenScreen";
import { AuditScreen, MeScreen, PermissionsScreen, TaxesScreen, UsersScreen } from "./screens/AdminScreens";
import { CashScreen } from "./screens/CashScreen";
import { Login } from "./screens/Login";
import { PosScreen } from "./screens/PosScreen";
import { PrintersScreen } from "./screens/PrintersScreen";
import { AvailabilityScreen, MenuScreen } from "./screens/MenuScreen";
import { SettingsScreen } from "./screens/SettingsScreen";
import { StockScreen } from "./screens/StockScreen";
import { PurchasesScreen } from "./screens/PurchasesScreen";
import { ReportsScreen } from "./screens/ReportsScreen";
import { CustomersScreen } from "./screens/CustomersScreen";
import { Setup } from "./screens/Setup";

interface Status {
  setup_done: boolean;
  shop_name: string;
  default_language: Lang;
  version: string;
}

type Page = "pos" | "floor" | "cash" | "kitchen" | "device" | "menu" | "printers" | "86" | "stock" | "purchases" | "reports" | "customers" | "settings" | "taxes" | "users" | "permissions" | "audit" | "me";

const NAV: { page: Page; label: Key; icon: ReactNode; perm?: string | string[]; phase?: number; color?: string }[] = [
  { page: "pos", label: "nav_pos", icon: <ShoppingCart />, perm: "pos.sell", color: "#1D4ED8" },
  { page: "floor", label: "nav_floor", icon: <LayoutDashboard />, perm: "settings.manage", color: "#C2410C" },
  { page: "cash", label: "nav_cash", icon: <Banknote />, perm: ["cash.session", "cash.in_out"], color: "#047857" },
  { page: "kitchen", label: "nav_kitchen", icon: <ChefHat />, perm: "kds.use", color: "#B45309" },
  { page: "86", label: "nav_86", icon: <Ban />, perm: ["menu.manage", "pos.sell", "kds.use"], color: "#B91C1C" },
  { page: "stock", label: "nav_stock", icon: <Boxes />, perm: ["inventory.manage", "kds.use"], color: "#A16207" },
  { page: "purchases", label: "nav_purchases", icon: <Truck />, perm: ["purchasing.manage", "inventory.manage"], color: "#0F766E" },
  { page: "customers", label: "nav_customers", icon: <Contact />, perm: "customers.manage", color: "#B45309" },
  { page: "reports", label: "nav_reports", icon: <BarChart3 />, perm: "reports.view", color: "#4338CA" },
  { page: "menu", label: "nav_menu", icon: <UtensilsCrossed />, perm: "menu.manage", color: "#7C3AED" },
  { page: "settings", label: "nav_settings", icon: <Cog />, perm: "settings.manage", color: "#374151" },
  { page: "printers", label: "nav_printers", icon: <Printer />, perm: "settings.manage", color: "#0E7490" },
  { page: "taxes", label: "nav_taxes", icon: <Percent />, perm: "settings.manage", color: "#4D7C0F" },
  { page: "users", label: "nav_users", icon: <Users />, perm: "users.manage", color: "#BE185D" },
  { page: "permissions", label: "nav_permissions", icon: <ShieldCheck />, perm: "users.manage", color: "#9333EA" },
  { page: "audit", label: "nav_audit", icon: <History />, perm: "audit.view", color: "#475569" },
  { page: "device", label: "nav_device", icon: <Wifi />, color: "#0369A1" },
  { page: "me", label: "nav_me", icon: <User />, color: "#64748B" },
];

function applyTheme(theme: "dark" | "light") {
  document.documentElement.classList.toggle("dark", theme === "dark");
}

export default function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [user, setUser] = useState<SessionUser | null>(null);
  const [dev, setDev] = useState<DeviceInfo | null>(null);
  const [displayExit, setDisplayExit] = useState(false);
  const [joining, setJoining] = useState(false);

  const loadStatus = useCallback(() => {
    loadDevice()
      .then((d) => {
        setDev(d);
        return rpc<Status>("app.status");
      })
      .then(setStatus, (e) => {
        // A Client whose Host is unreachable still starts: show the offline login.
        if ((e as { code?: string })?.code === "offline") setStatus({ setup_done: true, shop_name: "BURGER OS", default_language: "fr", version: "" });
        else setFatal(errorText("fr", e));
      });
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
  if (!status || !dev) return <p className="muted p-6">{translate(lang, "loading")}</p>;

  return (
    <LangContext.Provider value={lang}>
      {dev.mode === "client" && <OfflineBanner />}
      {dev.kind === "display" && !displayExit && status.setup_done ? (
        <CustomerDisplay shopName={status.shop_name} onExit={() => setDisplayExit(true)} />
      ) : !status.setup_done ? (
        <>
          <Setup onDone={loadStatus} />
          <div className="fixed bottom-4 end-4">
            <button className="panel min-h-12 rounded-xl px-4 font-semibold" onClick={() => setJoining(true)}>{translate(lang, "dev_setup_client")}</button>
          </div>
          {joining && <JoinDialog onClose={() => setJoining(false)} onJoined={() => location.reload()} />}
        </>
      ) : !user ? (
        <>
          <Login shopName={status.shop_name} lang={lang} onLogin={setUser} />
          <div className="fixed bottom-4 end-4 flex gap-2">
            <button className="panel min-h-12 rounded-xl px-4" onClick={() => { setDisplayExit(false); setDev({ ...dev, kind: "display" }); }}>{translate(lang, "disp_open")}</button>
          </div>
        </>
      ) : (
        <Shell
          user={user}
          setUser={setUser}
          onLogout={logout}
          onSettings={(s) => setStatus({ ...status, shop_name: String(s.shop_name), default_language: s.default_language as Lang })}
          version={status.version}
          defaultPage={dev.kind === "kds" ? "kitchen" : undefined}
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
  defaultPage,
}: {
  user: SessionUser;
  setUser: (u: SessionUser) => void;
  onLogout: () => void;
  onSettings: (s: Settings) => void;
  version: string;
  defaultPage?: Page;
}) {
  const t = useT();
  const can = (p?: string | string[]) => !p || (Array.isArray(p) ? p : [p]).some((x) => user.permissions.includes(x));
  const items = NAV.filter((n) => can(n.perm));
  // "home" = the app launcher grid (Odoo-style). A kitchen device opens straight on its screen.
  const [page, setPage] = useState<Page | "home">(items.find((n) => n.page === defaultPage)?.page ?? "home");
  const [searching, setSearching] = useState(false);
  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setSearching(true);
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, []);
  useAutoLock(onLogout);

  const current = NAV.find((n) => n.page === page);
  let body: ReactNode;
  if (!current) {
    body = <AppGrid items={items} onOpen={setPage} />;
  } else if (current.phase) {
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
      case "floor":
        body = <FloorScreen />;
        break;
      case "cash":
        body = <CashScreen me={user} />;
        break;
      case "menu":
        body = <MenuScreen />;
        break;
      case "customers":
        body = <CustomersScreen />;
        break;
      case "reports":
        body = <ReportsScreen />;
        break;
      case "purchases":
        body = <PurchasesScreen me={user} />;
        break;
      case "stock":
        body = <StockScreen me={user} />;
        break;
      case "86":
        body = <AvailabilityScreen />;
        break;
      case "kitchen":
        body = <KitchenScreen />;
        break;
      case "device":
        body = <DeviceScreen canManage={can("settings.manage")} />;
        break;
      case "printers":
        body = <PrintersScreen />;
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
    <div className="flex h-full flex-col">
      <StatusBar
        user={user}
        version={version}
        app={current ? t(current.label) : null}
        onHome={() => setPage("home")}
        onSearch={() => setSearching(true)}
        onLock={onLogout}
      />
      <main className="min-h-0 flex-1 overflow-auto">{body}</main>
      {searching && (
        <AppSearch
          items={items}
          onClose={() => setSearching(false)}
          onOpen={(p) => {
            setPage(p);
            setSearching(false);
          }}
        />
      )}
    </div>
  );
}

type NavItem = (typeof NAV)[number];

/** Home: every app the user may open, as big tiles. */
function AppGrid({ items, onOpen }: { items: NavItem[]; onOpen: (p: Page) => void }) {
  const t = useT();
  return (
    <div className="mx-auto grid max-w-5xl grid-cols-3 gap-x-6 gap-y-8 px-6 py-12 sm:grid-cols-4 md:grid-cols-6">
      {items.map((n) => (
        <button key={n.page} onClick={() => onOpen(n.page)} className="group flex flex-col items-center gap-2 rounded-lg p-2 hover:bg-[var(--panel)]">
          <span
            className="flex h-20 w-20 items-center justify-center rounded-2xl text-white shadow-sm transition group-active:scale-95 [&>svg]:h-9 [&>svg]:w-9"
            style={{ background: n.color }}
          >
            {n.icon}
          </span>
          <span className="text-center text-sm font-medium">{t(n.label)}</span>
        </button>
      ))}
    </div>
  );
}

/** Search box over everything: type part of an app's name, Enter opens it. */
function AppSearch({ items, onClose, onOpen }: { items: NavItem[]; onClose: () => void; onOpen: (p: Page) => void }) {
  const t = useT();
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const norm = (x: string) => x.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "");
  const hits = items.filter((n) => !q.trim() || norm(t(n.label)).includes(norm(q.trim())) || n.page.includes(norm(q.trim())));
  const pick = Math.min(sel, Math.max(hits.length - 1, 0));
  return (
    <div className="fixed inset-0 z-[80] flex items-start justify-center bg-black/50 px-4 pt-[12vh]" onClick={onClose}>
      <div className="panel w-full max-w-xl shadow-2xl" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={t("search")}>
        <label htmlFor="app-search" className="sr-only">{t("search")}</label>
        <input
          id="app-search"
          autoFocus
          value={q}
          placeholder={t("search_apps")}
          onChange={(e) => {
            setQ(e.target.value);
            setSel(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            else if (e.key === "ArrowDown") setSel((x) => Math.min(x + 1, hits.length - 1));
            else if (e.key === "ArrowUp") setSel((x) => Math.max(x - 1, 0));
            else if (e.key === "Enter" && hits[pick]) onOpen(hits[pick].page);
          }}
          className="h-14 w-full border-b line bg-transparent px-4 text-lg outline-none"
        />
        <div className="max-h-[50vh] overflow-auto py-1">
          {hits.map((n, i) => (
            <button
              key={n.page}
              onClick={() => onOpen(n.page)}
              onMouseEnter={() => setSel(i)}
              className={`flex h-12 w-full items-center gap-3 px-4 text-start ${i === pick ? "sel" : ""}`}
            >
              <span className="flex h-8 w-8 items-center justify-center rounded-md text-white [&>svg]:h-4 [&>svg]:w-4" style={{ background: n.color }}>{n.icon}</span>
              <span className="flex-1 font-medium">{t(n.label)}</span>
              {i === pick && <span className="num muted text-xs">↵</span>}
            </button>
          ))}
          {!hits.length && <p className="muted px-4 py-4">—</p>}
        </div>
      </div>
    </div>
  );
}

/** Top status line: product, register, user, connection, clock. */
function StatusBar({ user, version, app, onHome, onSearch, onLock }: {
  user: SessionUser; version: string; app: string | null; onHome: () => void; onSearch: () => void; onLock: () => void;
}) {
  const t = useT();
  const [now, setNow] = useState(new Date());
  const [dev, setDevState] = useState<DeviceInfo | null>(null);
  const online = useOnline(true);
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), 1000);
    rpc<DeviceInfo>("device.get").then(setDevState, () => {});
    return () => clearInterval(id);
  }, []);
  return (
    <header className="bar flex h-12 shrink-0 items-center gap-4 px-2 text-[13px]">
      <button onClick={onHome} aria-label={t("home")} title={t("home")} className="flex h-10 w-10 items-center justify-center rounded-md hover:bg-white/10">
        <LayoutGrid className="h-5 w-5" />
      </button>
      <b className={app ? "text-white" : "num text-white"}>{app ?? "BURGER/OS"}</b>
      <button onClick={onSearch} className="mx-auto flex h-9 w-full max-w-md items-center gap-2 rounded-md bg-white/10 px-3 text-start text-white/70 hover:bg-white/15">
        <Search className="h-4 w-4" />
        <span className="flex-1">{t("search_apps")}</span>
        <span className="num text-xs opacity-60">Ctrl K</span>
      </button>
      <span className="hidden lg:inline">{t("cash_register")} {dev?.register ?? "main"}</span>
      <span className="hidden md:inline">
        {user.name} · {t(`role_${user.role}`)}
      </span>
      {dev?.mode === "client" ? (
        <span className={online ? "text-emerald-400" : "text-red-400"}>● {online ? dev.host : t("err_offline")}</span>
      ) : (
        <span className="text-emerald-400">● {t("dev_mode_host").split(" (")[0]}</span>
      )}
      <span className="num hidden opacity-50 xl:inline">v{version}</span>
      <span className="num">{now.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })}</span>
      <button onClick={onLock} aria-label={t("lock")} title={t("lock")} className="flex h-10 w-10 items-center justify-center rounded-md hover:bg-white/10">
        <Lock className="h-4 w-4" />
      </button>
    </header>
  );
}

/** Client devices: red banner while the Host can't be reached, with the outbox count. */
function OfflineBanner() {
  const online = useOnline(true);
  const [queued, setQueued] = useState(0);
  useEffect(() => {
    const tick = () => rpc<DeviceInfo>("device.get").then((d) => setQueued(d.outbox), () => {});
    tick();
    const id = setInterval(tick, 3000);
    return () => clearInterval(id);
  }, [online]);
  if (online && !queued) return null;
  return (
    <div className="fixed inset-x-0 top-0 z-[70] bg-red-600 px-4 py-2 text-center font-semibold text-white" role="alert">
      {!online && translate((document.documentElement.lang as Lang) || "fr", "offline_banner")}
      {queued > 0 && ` · ${translate((document.documentElement.lang as Lang) || "fr", "offline_queued", { n: queued })}`}
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
