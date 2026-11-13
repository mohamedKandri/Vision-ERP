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

## Phase 2: menu, products, modifiers, combos (2026-11-13)

### Built
- Migration `0002_menu`:
  - Tables: stations (grill, fryer, drinks, assembly), categories (color and icon),
    products (kind simple/combo, price, happy-hour price, tax, station, photo, active,
    available/86), product_variants, modifier_groups (min/max), modifier_options
    (choice/extra/remove, price, max qty, default), product_modifier_groups, combo_slots,
    combo_slot_options (product, optional variant, extra price, default).
  - Names are stored as `{"fr","ar","en"}` JSON. French is required; the others fall back to French.
- `db::atomic()`: a nestable transaction guard on SAVEPOINT. It rolls back on early return or
  error, and every write path now uses it. (Phase 1's `settings::set_many` workaround is gone.)
- `menu.rs` (Rust) validates:
  - names, price range, existing category/tax/station, and photo (data: URL, ≤ 400 KB);
  - min ≤ max and a reachable minimum; "remove" options must be free;
  - combo rules: slots needed, one default per slot, no nested combos, a variant must belong
    to its product, and a product can't switch between simple and combo.
- Everything is soft-deleted (inactive), and variant/option ids stay stable, so past orders
  stay readable.
- Audit: product created/updated, **product.price_changed with before/after prices** (only when
  a price actually changes), category and modifier group saves, 86 / back in stock.
- Permissions: editing the menu needs `menu.manage`. The **86 toggle** is also open to `pos.sell`
  and `kds.use`, so a cashier or the kitchen can mark an item out of stock during service.
- Demo menu (`seed.rs`), optional at setup or with a "Load demo menu" button when the menu is
  empty. Every name is in FR/AR/EN:
  - 6 categories; **10 burgers**, most with Simple/Double/Triple variants;
  - 5 sides (fries in 3 sizes), 8 drinks, 4 desserts, 5 sauces;
  - 6 modifier groups: doneness, extras, remove, free sauces, combo size Maxi +10, milkshake flavour;
  - **4 combos** (burger + side + drink, with paid upgrades).
- `src/lib/menu.ts` (pure logic):
  - happy-hour window (can cross midnight) and base price with happy hour per product/variant;
  - default configuration and full validation (min/max, per-option qty, inactive options,
    options from another product, variants, combo slot choices, 86'd items);
  - radio vs multi-select toggling, unit price (combo + size + slot extras + paid extras on
    parts), and description lines for cart and kitchen tickets.
- UI:
  - **Menu** screen: products with category filter, search, reorder and preview; product editor
    with variants, modifier groups, combo slots, photo, and a live HT/VAT split under the price.
    Also categories and modifier groups.
  - **Ruptures (86)** board.
  - **Item configurator** popup. The POS reuses it in Phase 3.
  - Happy-hour section in Settings.

### Verified
- **Rust: 38 tests pass**:
  - Phase 1 tests, plus nested atomic rollback.
  - Name, category and group validation; product round-trip with variants and groups.
  - Stable variant ids with deactivation; price-change audit only on a real change; combo rules.
  - 86 and reorder; unknown references rejected.
  - Demo seed (6 categories, 10 burgers, 4 combos, AR+EN everywhere, a default in every slot).
  - API permissions: the cashier can't edit the menu, the kitchen can 86.
- **Vitest: 69 tests pass**: Phase 1 tests, plus names fallback, happy hour (day window,
  midnight crossing, disabled), HH price per variant, defaults, min/max/per-option errors,
  inactive or foreign options, 86, radio vs multi toggling, unit prices for simple and combo,
  and describe lines.
- `tsc` is clean and `vite build` succeeds.
- **End-to-end on the real app** (dev build, Playwright over the WebView2 DevTools port, fresh
  database). All checks below pass:
  - setup with demo staff and menu shows 36 products;
  - Classic Double + 2 cheese + no onion = **70,00 DH**;
  - Menu Classic + Maxi + Potatoes = **78,00 DH**;
  - changing a price writes `product.price_changed` with the old and new price;
  - a new category and product with an Arabic name save;
  - a group with min > max is refused by the server;
  - the cashier sees Ruptures but not Menu, and can mark Frites "Épuisé";
  - Arabic product names and statuses show on the 86 board;
  - no console errors.
- **Bug found and fixed by the E2E run:** new products defaulted to the first tax in the list
  (Exonéré 0 %) instead of the shop's default tax. They now use `default_tax_id`. Re-checked:
  40 DH shows "36,36 HT + 3,64 TVA".

### Not verified / known issues
- The Android APK has not been rebuilt for Phase 2, and nothing has run on a tablet yet.
- Photos are stored as JPEG data URLs (≤ 320 px) in SQLite. That's fine for a menu of this
  size; Phase 5 sync will need to send them efficiently.
- Recipes per product and modifier (stock) come in Phase 6. Price variants have no recipe link yet.
- Reorder arrows only show when a single category is selected, which is intentional.
