# BURGER OS — Dev log

Each phase records what was **verified** (by a test or by running the app) and what was **not**.

## Decisions taken before Phase 1 (with the owner)

| Topic | Decision |
|---|---|
| Repo | New folder `Desktop\BURGER-OS`, own git repo (VAULT untouched) |
| POS hardware | Generic Android tablet + ESC/POS LAN printer first; other printer types still supported later |
| Prices / VAT | Odoo-style: each tax has a rate and a "price includes tax" flag, chosen per product |
| Discounts | Every discount and comp needs a manager PIN (`discount_pin_threshold_bp = 0`, configurable) |
| Loyalty | Both stamps (Nth burger free) and points per MAD; picked in Settings |
| Order numbers | One running number, never reset (`sequences.order_number`) |
| Android testing | Real tablet over USB |

## Phase 1: setup, schema, settings, users/PIN login (2026-11-13)

### Built
- Tauri 2 + React 19 + TypeScript + Vite 7 + Tailwind 4, with app id `ma.burgeros.app`.
- SQLite (rusqlite, bundled) with WAL and foreign keys. Forward-only migrations run in one
  transaction each and are tracked in `schema_migrations`.
- Migration `0001_core`: settings, users, role_permissions, taxes (`rate_bp`, `price_include`),
  sequences, audit_log. Seeds 4 taxes and the default permission matrix.
- One IPC command, `rpc(token, method, params)`, routes to `api::dispatch`. The LAN server in
  Phase 5 will call the same function, so auth, permissions and audit rules live in one place.
- Auth:
  - PINs are 4–6 digits, hashed with Argon2id.
  - 5 wrong PINs lock that user for 60 s.
  - Session tokens live in memory, and the user is reloaded on every call, so deactivating a
    user or changing a permission takes effect immediately.
  - Manager override (`auth.authorize`) checks a PIN against users who hold the needed
    permission, and is audited.
- Permissions: 18 permissions across 4 roles. The Owner always has everything; only the Owner
  can edit the matrix. Nobody can create or edit Owners except an Owner. The last active Owner
  can't be removed, and users can't change their own role or deactivate themselves.
- Settings: an allow-list of 31 keys, each with a type and range. Changes are validated before
  anything is written, and saved atomically.
- `src/lib/money.ts` (pure logic, integer cents):
  - Rounding is half away from zero.
  - TTC/HT line totals, line discounts and order discounts. Order discounts are spread over
    lines by largest remainder, then VAT is recomputed.
  - Per-line or global VAT rounding, delivery fee with its own tax, change due, money parsing
    and formatting.
- UI:
  - First-run setup, with optional demo staff.
  - User tiles with a big PIN pad that also accepts the keyboard.
  - Settings, Taxes, Staff, Permissions matrix, Audit log and My profile (language, theme, change PIN).
  - Auto-lock after inactivity.
  - French, Arabic (RTL) and English, chosen per user; dark and light themes; touch targets of at least 48 px.

### Verified
- **Rust: 26 tests pass** (`cargo test`): migrations (fresh, idempotent, rollback on failure,
  persistence, FK enforcement), PIN rules, hashing, lockout, inactive users, owner permissions,
  override, audit writes, settings validation and atomicity, setup once, token required, cashier
  forbidden, manager can't touch owners, live permission change, deactivation kills the session,
  tax CRUD, change PIN.
- **Vitest: 52 tests pass**: divRound cases, VAT split (45.00 TTC → 40.91 + 4.09), HT vs TTC,
  discounts and caps, comps, allocation sums, per-line vs global rounding (3 × 1.05 at 10 % →
  VAT 0.30 vs 0.29), order discount spread with VAT recomputed, mixed HT/TTC, delivery fee,
  invariants over 400 random orders, change due, parsing, formatting, and identical i18n keys
  in all 3 languages.
- `tsc` is clean and `vite build` succeeds.
- **Windows release build + NSIS installer** (1.7 MB) built. It was installed silently for the
  current user and launched.
- **End-to-end on the real installed Windows app** (Playwright over the WebView2 DevTools port,
  clean database):
  - Setup with demo staff created 4 users.
  - Wrong PIN was rejected, then the owner logged in.
  - The ICE setting saved; adding "TVA 7 % HT" showed as `7 % · HT`.
  - A cashier was created and the permission matrix rendered.
  - Switching to Arabic + light set `dir=rtl`, `lang=ar` and removed the dark class.
  - The audit log contained setup.init, auth.login_failed, auth.login, settings.changed,
    tax.saved and user.created.
  - The cashier's menu showed only POS, Kitchen, My profile and Lock.
  - After 5 wrong PINs the UI showed "Trop d'essais. Réessayez dans 59 s", disabled the keypad
    and counted down. The DB showed `locked_until` set and 5 `auth.login_failed` rows.
  - No console errors.
- Android: `app-arm64-release` APK (6.8 MB) built through Gradle, then zipaligned and signed with
  the local debug key, and `apksigner verify` passed. Output: `release/burger-os-0.1.0-arm64-test.apk`
  (git-ignored).

### Not verified / known issues
- **The APK has not been installed on a real tablet yet.** That needs the tablet over USB.
- `tauri android build` fails on this PC at the symlink step (Windows Developer Mode is off).
  Gradle also failed with "Unable to establish loopback connection": the JDK's AF_UNIX pipe
  doesn't work in the default temp folder here. `scripts/build-android.ps1` works around both
  (it copies the `.so` and sets `-Djdk.net.unixdomain.tmpdir`). Turning on Developer Mode would
  also remove the first workaround.
- APK signing is not set up yet (planned for Phase 9). Until then, test APKs are signed with
  the local debug key.
- PIN check costs about 0.5–1 s per login with default Argon2 parameters on this PC. Not yet
  measured on a low-end tablet; parameters may need tuning.
- PINs are not forced to be unique. Checking uniqueness would tell a manager that a PIN belongs
  to someone else. Login picks the user tile first, so duplicates are harmless. A manager
  override takes the first matching manager.
- Daily automatic backup is only a setting so far. The backup itself is Phase 9.
