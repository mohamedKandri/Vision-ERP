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

## Phase 3: POS ordering, payments, cash sessions, receipt preview (2026-11-13)

### Built
- Migration `0003_orders`: dining tables (10 seeded), cash sessions (one open per register,
  enforced by a unique index), cash movements, orders, order lines, payments. Z numbers come
  from a sequence.
- **The Host computes all money**, so stored totals never depend on what a till sends.
  - `pricing.rs` is a Rust port of `money.ts`. `test-vectors/pricing.json` holds 306 cases
    (hand-written plus seeded random ones) and is checked by **both** Vitest and cargo test,
    proving identical totals to the centime.
  - `allocate` was rewritten with integer remainders in both languages to remove any
    float edge case.
  - `item.rs` is a Rust port of the menu rules in `menu.ts` (validate, unit price, ticket text
    in FR/AR/EN). The Host re-prices every line from the stored menu and the happy-hour clock.
- Orders (`orders.rs`):
  - Order types: dine-in with a table, takeaway with a name, delivery with phone, address
    and fee. The delivery fee is taxed at the default tax.
  - Order numbers never reset. A failed order rolls back without using up a number.
  - Lines snapshot names and ticket text, so menu edits never change past receipts.
  - Unsent lines can be edited or deleted. Lines already sent to the kitchen can only be
    voided, with a reason and the `pos.void` permission or a manager PIN.
  - Line and order discounts (% or fixed) and comps (with a reason). Approval follows
    `discount_pin_threshold_bp` (0 = always) using the `pos.discount` permission or a
    manager PIN typed at the till. The Host checks the PIN; a till can't fake an approval.
  - Payments by cash (amount received, change stored), card, or both. A card amount can't
    be more than what's due. Payments need an open till on that register.
  - Status flow: open → sent → preparing → ready → served; paid + served → closed.
  - Hold/park with a label; split by items onto a new bill numbered #N-2 (unpaid orders only).
  - Void (unpaid only), full refund (each payment method mirrored), reopen (manager).
    All are audited, and the receipt reprint is audited too.
- Cash (`cash.rs`):
  - Open the till with a float; cash in/out with a reason.
  - **X report** at any time: cash/card sales, refunds, expected cash, orders, average
    ticket, discounts, comps, voids, totals by order type, VAT by rate, movements.
  - **Close (Z)**: counted cash and the difference, a Z number, and a JSON snapshot frozen
    at close. Unpaid open orders produce a warning.
- `src/lib/receipt.ts`: receipt layout as printer-agnostic lines (logo, shop, ICE/IF/RC/Patente,
  order number, type/table/customer, items with details/notes/discounts/comps, subtotal, order
  discount, delivery fee, TOTAL TTC, VAT by rate, payments, change, balance due, footer, QR,
  DUPLICATA/REFUNDED/VOIDED marks) for 58 mm and 80 mm. Phase 4 prints this same list.
- UI:
  - **Till**: category tabs, a product grid (86'd items greyed out), the item popup, and the
    cart with quantity and notes. Totals estimate live with the same maths as the Host.
  - Order type, table picker and customer fields; send to kitchen; hold; discount; split;
    void/refund/reopen.
  - Manager-PIN prompt that retries the action; payment popup with a keypad, quick-cash
    buttons, card+cash split and a big change display; receipt preview; orders list.
  - Orders list with ready/served actions and a ready badge with a beep (polled every 10 s;
    the KDS drives it in Phase 5).
  - **Caisse du jour**: X report, cash in/out, Z close with a live difference, Z history.

### Verified
- **Rust: 56 tests pass**:
  - pricing: the 306 shared vectors, plus rounding basics;
  - items: Classic Double + 2 cheese = 70,00; Menu Classic Maxi + Potatoes = 78,00; combo
    slot and doneness rules; 86; happy-hour price;
  - orders:
    - numbering, Host-side totals and VAT, and a rejected order rolling back without using
      a number;
    - delivery rules and fee;
    - cashier discounts need a manager PIN (own PIN refused, override audited), the threshold
      setting, and comps needing a reason;
    - card + cash with change, overpay refused, and the paid + served → closed flow;
    - payment needs an open till;
    - sent lines need a void approval, unsent lines are deleted;
    - void/refund/reopen with audit, split numbering and totals, hold/resume;
  - cash: a full shift (sales, void, cash in/out, refund, X, Z with −5,00 difference, frozen
    Z, second session as Z n°2).
- **Vitest: 80 tests pass**: Phase 2 tests, the shared vectors, and 10 receipt tests (widths
  32/48, legal ids, items/details/notes, voided lines hidden, subtotal/discount/total/VAT/
  change, DUPLICATA/REFUNDED/balance due, AR/EN labels and names, delivery block and comps).
- `tsc` is clean.
- **End-to-end on the real app** (dev build, Playwright over the WebView2 DevTools port). It ran
  on the owner's own "test" database, with their permission. Checks were relative where
  existing data could affect them. Every check passes:
  - the till-closed banner, and opening the till with 500,00;
  - takeaway Coca + Classic Double + cheese + Frites Grande = **97,00**; paying with 100,00
    shows change **3,00**;
  - the receipt shows #1, 97,00, "TVA 10 % HT 88,18 / 8,82" and change 3,00;
  - the next order is #2;
  - dine-in at T3: Menu Classic + Tiramisu = 90,00, sent to the kitchen;
  - a 10 % item discount asks for a manager PIN; the cashier's PIN is refused and the
    manager's accepted, giving **83,50**;
  - splitting the Tiramisu onto #2-2 leaves **58,50**;
  - paid by card, then marked ready and served, the order leaves the active list (closed);
  - the #2-2 check is voided with a manager PIN;
  - the X report shows cash 97,00, card 58,50, expected **577,00** after a 20,00 cash-out,
    1 void, discounts 6,50, average ticket 77,75, and VAT 141,36 + 14,14;
  - reopening a paid receipt shows DUPLICATA;
  - closing the till with 577,00 counted gives Z with a difference of **0,00**;
  - no console errors.
- **Issue found by the E2E run and fixed:** the receipt VAT line used the tax's own name
  ("TVA 10% TTC HT 88,18"). It now shows "TVA 10 % HT 88,18 … 8,82".
- **Demo data bug found by tests and fixed:** the happy-hour price of Classic and Cheese was
  set on the product, but products with sizes take their price from the size, so it never
  applied. It's now on the Single size, and the editor explains that with sizes, happy hour
  is set per size.

### Not verified / known issues
- No printing yet (Phase 4). The receipt preview is the exact layout that will print.
  "Open drawer" only writes the audit entry for now.
- Refunds are full refunds only; there are no partial refunds.
- Split is by item and quantity, before any payment. Splitting an amount equally isn't
  done (card + cash split payment covers most cases).
- The ready badge polls every 10 s. Live push comes with the LAN WebSocket in Phase 5.
- The register name is per device (`localStorage`, default "main"). The device settings
  screen for choosing it comes with Host/Client in Phase 5.
- The Android APK has not been rebuilt for Phase 3, and nothing has run on a tablet yet.

## Phase 4: printing (2026-11-13)

### Built
- Migration `0004_printing`:
  - `printers`: type, address, paper 58/80, code page, graphic mode, cut, drawer, receipt
    register, ticket language;
  - station routing (`printer_id`, `show_on_screen`);
  - a durable `print_jobs` queue.
- `escpos.rs` (ESC/POS encoder):
  - Code pages CP437/850/858/1252/ASCII, with accent fallback and normalised spaces/dashes.
    French money formatting uses narrow no-break spaces, which print as plain spaces.
  - Bold, big text and alignment; wrap/row use the same algorithm as the on-screen preview.
  - Native QR (`GS ( k`); PNG logos and full-page images converted to `GS v 0` raster in
    256-row bands (logos dithered, text images thresholded); partial cut.
  - Drawer kick: generic `ESC p` or **SUNMI `DLE DC4 0 0 0`** (the command SUNMI documents).
- Transports:
  - **Network** TCP 9100 (3 s connect timeout).
  - **USB on Windows** through the spooler in RAW mode (`windows-sys` winspool), with
    printer discovery.
  - **Bluetooth on Android** through a Kotlin Tauri plugin (`PrinterPlugin.kt`): RFCOMM SPP,
    with a channel-1 fallback for low-cost printers and the `BLUETOOTH_CONNECT` permission.
  - **SUNMI built-in printer** through the same Bluetooth path, to the paired virtual
    printer "InnerPrinter" (docs.sunmi.com, printer development).
- Kitchen tickets (`printing.rs`):
  - Queued **in the same transaction** that sends the lines, so a ticket can't be lost.
  - One ticket per station that has a printer. **Combos are split into their parts**:
    burger → Grill, fries → Fryer, drink → Drinks, each tagged with the combo and its size.
  - Ticket layout: big order number, NEW / ADDED / CANCELLED / REPRINT header, type,
    table or name, time, items with details and notes ("!!"), and the order note.
  - **Changed orders print only the changes**: items added later print an "ADDED"
    ticket, and voiding an item already sent prints a "CANCELLED" ticket.
  - French or English per printer.
- Queue worker: a background thread that sends one printer at a time, in order, with
  printers handled in parallel. Failed jobs retry with backoff (2…30 s) and wake up as
  soon as a new job is queued.
  - Jobs leave the queue only when printed, or when a manager deletes them with a PIN.
  - A manager can **reroute** a dead printer's jobs to another printer.
  - The till shows a red banner with retry when any job has failed.
- Receipts and reports:
  - The till prints the same receipt layout it previews, automatically after payment
    (setting).
  - Paying cash kicks the drawer (setting), queued as its own job before the receipt.
  - X/Z reports print as well (`reportDoc`, tested).
  - **Arabic receipts and "graphic mode" printers** are rendered on a canvas by the till,
    which shapes Arabic and lays it out right-to-left, then printed as an image.
- `rpc` now runs off the UI thread (async + `spawn_blocking`), so PIN hashing or a slow
  Bluetooth printer can't freeze the screen.
- UI: **Imprimantes** screen (add/edit, discover USB/Bluetooth, test print with result,
  station routing, queue with retry/delete/reroute); the till's print/reprint, kitchen
  reprint and drawer buttons; X/Z print; three new switches in Settings → Sales.

### Verified
- **Rust: 80 tests pass**, including:
  - **Encoder:** code page bytes (é→0x82 in CP850, €→0xD5/0x80), tables of 128 entries,
    wrap/row matching the preview, styles/rule/cut bytes, big text wrapping at half width,
    generic and SUNMI drawer bytes, native QR, PNG→raster (exact bits; scaling and
    banding; RGBA with transparency), bad images skipped without stopping the print.
  - **Printing logic:** address validation; one receipt printer per register; combo split
    across stations; ADDED/CANCELLED/REPRINT tickets showing only changes; no printers or
    setting off → no jobs; retry order and backoff (the second ticket waits behind the
    first); reroute; the drawer job needs a drawer.
  - **Transports:** exact bytes over a **real TCP socket**; a refused connection is
    reported; **Windows printer list works on this PC**.
  - **Full pipeline test through the API** with the real worker and two fake network
    printers:
    - the cashier can't manage printers;
    - the Grill gets "2 x Kefta Burger … NOUVELLE … !! bien cuit" (no Coca), cut;
    - paying in cash sends the drawer kick, then the receipt;
    - with the grill printer unreachable, the ADDED ticket fails and shows as a problem;
    - after a retry it prints, and nothing was lost;
    - drawer button and kitchen reprint (`RÉIMPRESSION` in CP850) work.
- **Vitest: 83 tests pass** (adds the X/Z report layout: values, VAT, movements, fits 58 mm).
- `tsc` is clean and `cargo build` (desktop) succeeds.
- **Arabic receipt image checked in a real browser** (canvas): shaped Arabic, right-to-left
  layout and QR. Two flaws were found and fixed: the currency mark printed as ".د.م" (amounts
  are now drawn right-to-left) and an amount was split from its currency across two lines
  (they're now kept together).

### Not verified / known issues
- **No real printer, Bluetooth device or SUNMI terminal has been tested yet.** Network
  printing is proven against TCP sockets only. Bluetooth and SUNMI code paths have only
  been compiled (see Android build below).
- The owner chose not to run the UI end-to-end on their database this phase. The printing
  screens were type-checked and run in the dev app, but not driven by a script.
- Kitchen tickets print in French or English. Arabic kitchen tickets would need the Host
  to render images; not done.
- Some printers only use the generic `ESC p` drawer command on pin 5; there's no setting
  for that yet.
- USB printing needs the printer installed in Windows (any driver; data is sent RAW).
- **Android build:** the APK builds with the Kotlin Bluetooth plugin (Gradle BUILD SUCCESSFUL),
  is signed with the debug key (`apksigner verify` passes), and its manifest contains the
  BLUETOOTH, BLUETOOTH_ADMIN and BLUETOOTH_CONNECT permissions. Output:
  `release/burger-os-0.1.0-arm64-test.apk` (7.6 MB). It has not been run on a tablet yet.

## Phase 5: Host/Client LAN, kitchen screen, statuses, customer screen (2026-11-13)

### Built
- Migration `0005`:
  - `devices`: paired devices; only a SHA-256 of each device token is stored.
  - `kitchen_parts`: one row per item per station; a combo is split into its parts.
  - `outbox`: Client side, for orders made while offline.
- **Device role** (`net.rs`, stored in `device.json` next to the database): Host or Client,
  name, kind (till / kitchen screen / customer screen / office), register, port (7420).
- **Host server** (`server.rs`, axum), on the LAN at port 7420:
  - `/ping`, and `/pair`, which needs the pairing PIN;
  - `/rpc` and the `/ws` live channel, which need a paired device token;
  - user permissions on `/rpc` are checked by the same `dispatch` as on the Host itself.
- **Pairing**:
  - The owner opens a 10-minute window, which shows a QR code and a 6-digit PIN.
  - One device per window; 5 wrong PINs close it.
  - The Owner can revoke a paired device at any time.
- **Live updates**: every successful change publishes a topic (orders, menu, cash, print,
  settings…). The Host's own screen gets it as a Tauri event; Clients get it over the WebSocket.
  Screens reload what they show, with slow polling as a safety net.
- **Client** (`client.rs`):
  - The Rust side forwards every call to the Host. There are no browser restrictions, and
    the same code runs on Android.
  - The connection drops and comes back automatically, and a red **Offline** banner shows
    while the Host is unreachable.
  - **New orders made while offline go to a local outbox** and replay in order once the
    Host is back. Payments and edits to existing orders need the Host.
- **Kitchen Display** (`kds.rs` + Kitchen screen):
  - Parts are created in the same transaction that sends items to the kitchen.
  - The screen filters by station and shows the order number, type/table/name, the items
    with details, and notes highlighted.
  - The timer turns orange or red after the configured minutes.
  - Tap an item to mark it done; **Terminé** bumps the ticket for this station;
    **Rappeler le dernier** recalls the last bumped ticket. A sound plays on new tickets.
  - When every station is done, the order becomes **Ready**. The till then beeps and shows
    "Commande #N prête !".
  - Voided items and orders leave the screen; ready/served on the till clears it.
  - Stations without a screen (print-only) are skipped.
- **Customer screen**: big "En préparation" and "Prête !" columns of order numbers.
  - It's public and shows only numbers.
  - It opens from the login screen, or by default on a device of the "customer screen" kind.
- **Device screen**:
  - On the Host: its addresses, the pairing window, and the paired devices with revoke.
  - On a Client: join or leave a Host.
  - On a fresh install, "This device joins an existing shop" is offered next to setup.
  - The register name now comes from the device's settings.

### Verified
- **Rust: 88 tests pass**, including:
  - KDS: station routing with Arabic names, tap/undo, bump → preparing → ready, display
    board, recall, voids, served clearing the screens, print-only stations skipped.
  - Pairing rules: wrong PIN, wrong QR code, one device per window, 5 tries close it.
  - Device tokens: stored hashed, revocable. Topic mapping.
  - **Two devices over real HTTP + WebSocket** (`lan_test.rs`, real axum server on a socket):
    - `/ping`; the cashier can't open pairing;
    - wrong PIN refused, then pairing works;
    - no device token or a forged one → refused, for HTTP and for the WebSocket;
    - remote cashier login keeps the same permissions (`settings.set` forbidden);
    - an order sent from the "tablet" pushes "orders" over the WebSocket;
    - a remote kitchen user can't sell, and bumping the drinks and assembly stations makes
      the order ready and puts its number on the public customer board;
    - a revoked tablet is cut off;
    - **offline**: a new order goes to the outbox while a payment is refused as offline;
      after re-pairing and logging in, the outbox replays and the order exists on the Host.
- Vitest 83 pass; `tsc` clean.
- **The real app** (dev build) answers `/ping` on 127.0.0.1:7420 and on its LAN address, and
  refuses `/rpc` without a paired device ("device_unknown").
- Android: the Rust side (axum, tokio, ureq, tungstenite) compiles for aarch64, and Gradle
  BUILD SUCCESSFUL.

### Not verified / known issues
- **No second physical device yet** (the owner is on a work laptop). Real Wi-Fi between a
  tablet and the PC, the Windows firewall prompt, and Android network permissions are not
  tested. Two app windows can't run on one PC because they would share the same data folder.
- The UI screens (device, kitchen, customer) are type-checked and the dev app starts, but
  they were not driven by a script this phase.
- The Client types the Host address and PIN by hand. **Scanning the QR with the tablet
  camera is not done**; pasting the QR text into the address field works.
- LAN traffic is plain HTTP inside the shop network (no TLS). Each device needs a pairing
  token, and each user still logs in with their PIN.
- Offline mode covers new orders only. Payments, edits to existing orders, and closing the
  till need the Host.
- Host sessions live in memory, so after a Host restart the users on the tills log in again.

## Printer output formats (HTTP / base64)

- New printer type **HTTP (URL)** with format **ESC/POS raw** (POST `application/octet-stream`) or **ESC/POS base64** (POST JSON `{ <field>: base64, printer, bytes }`). JSON field name and optional `Authorization` header are configurable. Only `http://` (no TLS bundled).
- Migration 0006 rebuilds `printers` (kind check + `format`, `http_field`, `http_auth`); stations routing and the one-receipt-printer-per-register rule are kept.
- The Authorization value is never returned to the screen (`has_http_auth` only) and not written to the audit log; editing without retyping keeps it.
- Verified: Rust tests 92 pass (migration keeps printers + routing; local HTTP server receives raw body, base64 decodes to the exact ESC/POS bytes, auth header sent, HTTP 503 fails the job; validation). Typecheck + 83 Vitest pass.
- Not verified: a real HTTP print bridge/printer; the edited Printers screen in the running app.

## Floor plan and tables

- New **Plan de salle / Floor plan** app (permission `settings.manage`): floors (rooms), tables on a 24 x 16 grid; drag to place, tap to edit name, seats, shape (square/round), size, floor, active. Layout save is all-or-nothing.
- POS table button now opens the live floor plan: free / occupied / ready / paid colours, amount due, order number and minutes open. Tap a free table to seat the new order, tap a busy one (with nothing in progress) to open its order; with a saved dine-in order open, tapping a table moves the order (and its split parts) there (`orders.move_table`, audited).
- Migration 0007: `floors`, table position columns; existing tables laid out automatically (Terrasse… on the terrace floor). Ran on the real app DB at start: version 7, all 10 tables placed.
- Rules: unique table names, tables must fit on the board, unknown floor rejected, can't deactivate a table with an open order, can't delete a floor that has tables, closed orders can't move.
- Verified: Rust 95 tests pass (3 new floor tests), typecheck, Vitest 83 pass.
- Not verified: the screens by eye / by touch (drag on a real tablet), RTL layout of the board.
- Caisse now opens on the floor plan (when tables exist): tap a free table to start its order, a busy one to open it, or Takeaway / Delivery. "Tables" button on the till goes back; finishing or starting a new order returns to the plan. Typecheck + Vitest 83 pass; not checked by eye.
- A table is free again as soon as its orders are paid (encaissé), even while the kitchen still serves; the "paid" colour is gone.
- Caisse floor screen: "Edit plan" (managers, `settings.manage`) opens the editor in place: drag tables, add / edit / delete tables and floors, "Done" returns to service. Delete removes a never-used table, or takes a used one off the floor and keeps it for order history; refused while an order is open on it.
- Verified: floor Rust tests (3) pass incl. paid-frees-table and delete rules; typecheck; Vitest 83. Not checked by eye.

## Phase 6: inventory, recipes, automatic stock deduction, waste, stock counts (2026-11-16)

### Built
- Migration `0009_inventory`:
  - `ingredients`: unit g / ml / piece; cost per kg, per litre or per piece; a minimum.
    Quantities are integers in thousandths of the unit (150 g = 150000), so half a piece works
    and no float ever touches stock.
  - `recipe_lines`: what one unit of a **product**, a **size** or a **modifier option** uses.
    A size with its own recipe replaces the product's (Double = 300 g of beef). An option
    adds (Extra cheese = +1 slice) or removes ("No onion" = −20 g of onion; never below 0).
  - `stock_moves`: the ledger. Kinds: sale, return, waste, adjust, count, purchase (Phase 7),
    initial. Each move stores its value at the cost of the moment.
    `ingredients.stock_milli` is a cache of that ledger, updated in the same transaction.
  - `stock_counts` / `stock_count_lines`. `order_lines.stock_per_unit`. `products.stock_out`.
- `stock.rs`: pure functions (consumption of a configured item, combos included; cost;
  margin; price excl. VAT; ledger diff) and the DB layer.
- **Automatic deduction** (`stock::sync_order`), called inside the same transaction as every
  order change: send, pay, edit, remove/void a line, void, refund, split.
  - A line uses stock once it goes to the kitchen, or once the order is fully paid (takeaway
    paid before sending).
  - It is idempotent: it compares what each line should have used with what the ledger
    already has for that line, and writes only the difference. Calling it twice does
    nothing.
  - What a line uses per unit is **frozen the first time**, so editing a recipe never
    rewrites past sales.
  - Void (line or order) gives the stock back when **"Restore stock on void"** is on (default
    on). Otherwise, what was cooked stays used. A refund gives it back when the new setting
    **"Restore stock on refund"** is on (default off: the food was eaten).
  - Split checks keep the ledger right. A line deleted after using stock is given back.
- **Automatic 86** (setting "auto_86_on_stockout", default off): when an active ingredient
  reaches 0, every product whose recipe (or one of its active sizes) uses it is marked out of
  stock. It comes back by itself on restock. Products 86'd by hand are never touched, and
  changing availability by hand clears the automatic flag. The till's menu is refreshed live.
- Waste: per ingredient, or per prepared product (its recipe is taken out), with a reason.
  Adjustments (in/out) need a reason. Both are in the audit log, with their value.
- Stock counts: one open at a time. The sheet lists every active ingredient; only lines that
  were filled in are corrected on validation, against the stock at that moment (sales made
  during the count are taken into account). The expected quantity is stored.
- Food cost: for every product and size (and each combo with its default choices): price
  excl. VAT, recipe cost, food cost % and margin %.
- API: `ingredients.list/save`, `stock.alerts/adjust/waste/history/food_cost/seed_demo`,
  `recipes.list/save`, `counts.list/get/start/set/validate/cancel`. Live topic `stock`.
  - Permissions: `inventory.manage` for everything. Users with `kds.use` or `pos.sell` can
    read stock levels. `kds.use` can record waste (cashiers have it by default). Food cost
    needs `inventory.manage` or `reports.view`.
- Demo data: 49 ingredients with costs and opening stock, recipes for all 37 demo products
  (Simple / Double / Triple, fries sizes, combo bag), extras, removals, free sauces, Maxi,
  milkshake flavours. Seeded by "setup with demo menu", or from the Stock screen when there
  are no ingredients yet (products matched by their French name; missing ones are skipped).
- UI: **Stock** app with four tabs:
  - **Stock**: alerts, search, low only, value, add/edit, adjust, waste.
  - **Recipes & food cost**: list by category with a margin badge; recipe editor with live
    cost and margin; options with + / − quantities.
  - **Stock counts**: sheet with search, difference per line and value; validate after a
    confirmation.
  - **Movements**: filter by ingredient and type; order number, product, reason, user.
  - Kitchen users only see the list and "Record waste".
  - French, English and Arabic. Settings → Sales gains "Restore stock on refund".
- Debug builds only: `BURGER_OS_DATA_DIR` points the app at another data folder (used to test
  the screens on a throwaway database). Release builds ignore it.

### Verified
- **Rust: 106 tests pass** (11 new in `stock.rs`):
  - consumption rules: size replaces product, extras ×qty, removals clamp at 0, combos;
  - cost rounding: 150 g at 90/kg = 13.50, half a piece; tiny amounts are summed before
    rounding;
  - price excl. VAT and margin (45.00 incl. 10% → 40.91; negative margin under cost);
  - demo seed: every product has a recipe; Classic Simple costs **18.71**; Double = +13.50;
  - sales through the real order functions:
    - nothing used before sending or paying;
    - sending uses stock, and paying afterwards doesn't use it twice;
    - paid-without-sending uses stock;
    - Double + 2 extra cheese + no onion are counted exactly;
  - voiding a line or an order returns stock; with the setting off it stays used; refund
    follows its own setting;
  - a recipe change doesn't rewrite a past sale; a split moves nothing;
  - waste (ingredient and product), adjustments, reason required;
  - count: only counted lines change, one open count at a time, closed count is locked,
    expected stored; history in order;
  - automatic 86 when the last kefta is used, back on restock; manual 86 is respected;
    setting off does nothing;
  - validation: unit locked once used, duplicate or negative product lines, unknown
    ingredient, low alert;
  - through `dispatch`: setup with the demo menu seeds ingredients; cashier can't edit
    ingredients; kitchen can't adjust or count; waste shows who did it and the value;
    topics;
  - after every scenario: cached stock = sum of the ledger.
- **Vitest: 90 pass** (7 new in `stock.test.ts`: quantity parsing/formatting, the same cost
  and margin numbers as Rust, including Classic = 18.71). `tsc` clean.
- **The real app, driven by a script, on a throwaway database** (separate debug build,
  `BURGER_OS_DATA_DIR`, separate WebView2 profile; the shop's database wasn't touched):
  30 checks pass, no page errors.
  - Setup with the demo menu, then 2 Kefta Burgers sent to the kitchen. The Stock list shows
    198 buns and 4.7 kg of kefta.
  - Waste of one Kefta Burger from the dialog: 197 buns.
  - +5 kg beef delivery shows 25 kg.
  - Raising Avocado's minimum gives the "low" badge and the banner.
  - Food cost: Classic Simple shows 40.91 excl. VAT, 18.71 cost, 54.27 % margin.
  - The recipe editor shows 18.71. Adding a cheddar slice shows 19.91 live, and the list
    has it after saving.
  - "No onion" shows −20 g.
  - Count: Coca counted 90 shows −6; the confirmation says 2 counted / 2 corrected; then
    "Validated".
  - Movements show every type, the waste reason and product, and the order number.
  - Arabic: the page is right-to-left with Arabic ingredient names.
  - The kitchen user sees no recipes, adjustments or values, but can record waste.

### Not verified / known issues
- Not tried on a tablet by touch, and the APK was not rebuilt this phase (no Android-only
  code changed).
- Supplier deliveries arrive in Phase 7 (`purchase` moves); until then use an adjustment
  "Delivery".
- Ingredient cost is the current cost (no weighted average yet; Phase 7 updates it on
  receipt). Past moves keep the value they had.
- Automatic 86 ignores combos and modifier options: a combo whose default burger is out
  stays on sale, and the till still checks each choice.
- Stock can go negative (a sale is never blocked); it shows in red.
- Your dev app restarted during this phase and ran migration 9 on your own "test" database.
  The migration only adds empty tables and two columns: no data was changed and no
  ingredients were added there. "Load demo ingredients and recipes" in Stock adds them.

## Phase 7: suppliers and purchase orders (2026-11-16)

### Built
- Migration `0010_purchasing`:
  - `suppliers` (name, phone, email, address, note, active);
  - `supplier_items`: what a supplier sells, as a pack ("Sachet 5 kg") with its last price,
    and one **preferred** supplier per ingredient (unique partial index);
  - `purchase_orders` + `purchase_order_lines` (number BC-n from a sequence);
  - `purchase_receipts` + `purchase_receipt_lines`;
  - `stock_moves.po_id`.
- `purchasing.rs`:
  - Pure functions: cost per kg / L / piece of a delivery, **weighted average cost**, value
    of a partial pack, order status, packs to suggest.
  - Order flow: draft (editable) → ordered (locked) → partially received → closed.
    Cancel works while nothing has been received; after that, "Close" ends the order and
    the rest is no longer expected.
  - **Goods receipt** (all or part of an order, at the real price if it changed):
    - adds `purchase` stock moves valued at what was paid, with the reason "BC-n";
    - sets the cost to the weighted average of the stock on hand and the delivery (the
      delivery price when stock is 0 or negative);
    - remembers the supplier's new pack price;
    - updates the order status; can bring auto-86'd products back; audited.
  - **Suggestions**: active ingredients at or under their minimum.
    - Target: twice the minimum, in whole packs, minus what is already on order (drafts
      included).
    - Supplier: the preferred one, else the cheapest per unit.
    - "Create purchase orders" makes one draft per supplier.
  - Demo: 4 suppliers (butcher, bakery, greengrocer, wholesaler) with packs and prices for
    the demo ingredients. Seeded together with the demo stock.
- API:
  - `suppliers.list/save`;
  - `purchases.list/get/save/order/receive/close/cancel/suggestions/from_suggestions`;
  - live topic `stock`.
  - Permissions: `purchasing.manage` for everything. `inventory.manage` can also see orders
    and receive goods.
- UI: **Achats / Purchasing** app.
  - **Purchase orders**:
    - open / all lists;
    - editor: the supplier's items first; pack, size, packs, price, total;
    - detail with status, ordered and received per line, and the receipts;
    - receipt dialog: prefilled with what is still expected, price editable with the new
      cost per unit, delivery-note number.
  - **To order**: the suggestions, with editable packs and a total.
  - **Suppliers**: cards and an editor with their items and the preferred flag.
  - French, English and Arabic. Stock → Movements shows "Réception · BC-n".

### Verified
- **Rust: 111 tests pass** (5 new):
  - pure rules: 10 kg for 900 → 90/kg; average 5 kg at 90 + 5 kg at 100 → 95; zero or
    negative stock takes the new price; 100 buns at 2.50 + 24 at 2.75 → 2.55;
    partial-pack value and rounding; status; suggested packs (whole packs, minimum
    reached, already on order);
  - full flow on the demo data:
    - 4 × 5 kg of beef ordered, so a draft can't be received;
    - once ordered it can't be edited;
    - 10 kg received at 500/5 kg → partial, stock 30 kg, cost (20×90 + 10×100)/30 =
      93.33, supplier price updated;
    - the rest received at the ordered price → closed;
    - moves of 1000.00 and 900.00 linked to BC-1; ledger = cache;
  - close / cancel / validation: bad date, unknown supplier, empty order, cancel after a
    receipt refused (close works), a line from another order refused, "open" filter;
  - suggestions:
    - 30 buns (minimum 40) → 3 packs of 24 from the bakery;
    - a new preferred supplier takes over, with only one preferred kept;
    - once on order, buns are no longer suggested;
    - a supplier that doesn't sell the item is refused;
  - supplier validation;
  - API: the cashier can't see purchasing; setup with the demo menu seeds 4 suppliers;
    save → order → full receipt = closed; topics.
- **Vitest: 93 pass** (3 new in `purchasing.test.ts`, same numbers as Rust). `tsc` clean.
- **The real app, driven by a script, on a throwaway database** (`BURGER_OS_DATA_DIR`): 22
  checks pass, no page errors.
  - Suggestion: 30 buns → bakery, 3 packs = 180.00. The draft is created and opened.
  - Order placed. The receipt is prefilled with 72 buns. Receiving 48 at 66.00 per pack
    shows 132.00 and 2.75 per bun.
  - After the receipt: partial, the delivery note is listed, stock 78, cost 2.65. Then
    closed, and listed under All.
  - New supplier "Laiterie Atlas" with a preferred 6 L milk pack. In a new order its item
    comes first; 5 packs = 240.00; saved as draft BC-2.
  - Stock → Movements shows "+48 pce · Réception · BC-1 · 132.00".
  - The kitchen user has no Purchasing app. Arabic is right-to-left.

### Not verified / known issues
- Not tried on a tablet by touch; the APK was not rebuilt (no Android-only code changed).
- Purchase orders can't be printed or sent to the supplier yet (they're read out or copied
  by hand). PDF/CSV export comes in Phase 8.
- A closed order's total is the ordered amount; the received amount shows on its detail.
- Purchase VAT isn't tracked: prices are entered excluding VAT.
- Your dev build was rebuilt with migration 10 during this phase. If your app was started
  afterwards, it added the empty purchasing tables to your test database (no data
  changed). Demo suppliers are only added together with the demo ingredients ("Load demo
  ingredients and recipes" in Stock).

## Phase 8: reports, dashboard, CSV/PDF export, audit log (2026-11-23)

### Built
- `reports.rs`, one report for any date range, computed from the ledger (no new tables, no
  migration). The same counting rules apply everywhere, so every section adds up:
  - a **sale** counts when the order is fully paid (`paid_at`), with its current totals;
  - a **refund** counts on the day it is made, as the same amounts negated;
  - voided orders are never sales; they are listed under voids;
  - **food cost** of a line = the net value of its stock moves (used minus given back). A
    refund removes the revenue but not the cost (if the stock wasn't given back, the food was
    made and is a loss).
- Sections: totals (orders, gross, discounts, refunds, net TTC/HT, VAT, delivery fees,
  average ticket); by local hour and day; by product and category with cost, margin and
  food cost %; by order type; by payment method (money actually taken, from `payments`); by
  staff (who took the order); VAT by tax; every discount and comp with reason and approver;
  voided orders, lines voided after sending, refunds; food cost and margin; stock value now
  plus the period's moves by kind; waste by ingredient and entry by entry.
- Dates: the screen sends its local days as UTC instants plus its UTC offset, used for
  grouping by local hour and day. Validated format; at most ~400 days.
- API: `reports.summary` (`reports.view`). `audit.list` gains filters: date range, user,
  action groups (prefix match, no LIKE wildcards) and free-text search (action, entity id,
  details, user name); up to 5000 rows for export.
- Export files are written **on the device that asks** (`device.save_export`, never forwarded
  to the Host), to `Downloads/BURGER-OS`, else the app's own `exports` folder. File names are
  checked (letters, digits, space . - _, `.csv`/`.pdf` only); an existing file is never
  overwritten ("name (2).csv").
- UI: **Rapports / Reports** app (`reports.view`).
  - **Today**: net sales, orders, average ticket, discounts and refunds, each compared with
    yesterday at the same time; sales by hour (bars, tap for the value); top 5 products;
    payment methods; order types. Refreshes live.
  - **Date range**: presets (today, yesterday, this week from Monday, last 7 days, this month,
    last month, last 30 days) or two dates; every section as a table; **CSV** and **PDF**.
  - CSV: one file, every section stacked, UTF-8 with BOM; `;` and decimal comma in
    French/Arabic, `,` and point in English; text that starts with = + - @ is quoted so Excel
    doesn't run it.
  - PDF: A4 pages drawn on a canvas (the browser shapes Arabic) and packed as JPEG images by
    a small PDF writer (`lib/pdf.ts`), so no font or library is added. In Arabic, numbers are
    isolated left-to-right ("د.م. -12,00").
  - **Journal / Audit log**: filters (dates, who, action group, search) and CSV export.
  - French, English, Arabic.

### Verified
- **Rust: 118 tests pass** (7 new):
  - pure rules: date/instant validation, range limit, time-zone modifier, calendar
    arithmetic, average and percentage rounding, VAT merge with refunds, a line's own
    discount;
  - sales, refunds, VAT and groupings: 3 sold, 1 refunded, 1 unpaid, 1 voided. Net 71.00,
    and by hour, day, type, staff, product + delivery fee, category and VAT all add up to it.
    Payment methods: cash 99.00 in, 50.00 out. Food cost equals the stock the sold lines
    used;
  - local hours and days at UTC+1 across midnight; an empty range gives zeros;
  - discounts: 10 % line, comp, 5.00 order discount; listed amounts = the orders' discount
    total. A line voided after sending, with its reason; waste by ingredient;
  - API permissions (cashier refused, manager and owner allowed); audit filters, including
    `%` treated as text; export file names and no overwrite.
- **Vitest: 105 pass** (12 new in `report.test.ts`): date presets (week from Monday, last
  month across a year), day parsing, file names, CSV numbers/escaping/formula guard/BOM,
  report tables, percent format and no "-0,00", PDF structure (every xref offset checked),
  Arabic number isolation, base64. `tsc` clean.
- **The real app, driven by a script, on a throwaway database**: 26 checks pass, no page
  errors.
  - Today: net 71.00, 4 orders (the comped order paid at 0.00 counts), average 30.25,
    refund shown, top products, hour bars.
  - Date range: summary, VAT, payment methods, voids and refunds with reasons, comp listed,
    food cost % in French format; last month shows "nothing in this period".
  - CSV and PDF saved and opened: BOM, `;` and decimal comma, all sections; the PDF has 3
    pages (looked at in French and Arabic).
  - Audit log: the "Orders" filter, the search "plainte" finds the refund, the CSV holds
    only the filtered rows.
  - The cashier has no Reports app and the Host refuses `reports.summary`. Arabic is
    right-to-left.
- Found and fixed during the UI check: percentages showed a decimal point in French ("79.34
  %"), a zero refund showed "-0,00 DH", Arabic PDF amounts lost their order, the PDF header
  repeated the title, and the hour chart drew one very wide bar early in the day.

### Not verified / known issues
- Not tried on the tablet. Saving to `Downloads/BURGER-OS` on Android is untested: if
  Android refuses it, the file goes to the app's own folder and the message shows where.
  There is no "share" or "open" button yet.
- Reports use each order's current totals: a paid order that is reopened and changed later
  changes past reports (the Z report keeps its frozen snapshot).
- A refund on a later day lowers that day's sales, not the original day's.
- The PDF is made of page images: its text can't be selected or searched.
- Food cost is 0 for products without a recipe, and for sales made before Phase 6.
- Times use the device's current UTC offset for the whole range; a range across a clock
  change groups the hours after the change one hour off.
- No database change in this phase: your test database is not touched by this phase.

## Phase 9: customers, loyalty, backups, security review, Arabic check, release builds (2026-11-23)

### Built
- **Customers** (migration `0011_customers`, `customers.rs`):
  - one customer per phone number, normalised ("+212 6..", "00212 6.." and "06 .." are the same);
  - a new order with a phone links to the customer (created if new); a name or address typed
    for the order becomes the customer's latest one;
  - API `customers.lookup` (till), `customers.list/get/save` (`customers.manage`);
  - **Clients** app: search, edit, orders, loyalty history, balance correction.
- **Loyalty** (Settings: none / stamps / points), ledger `loyalty_moves`:
  - stamps: products flagged `loyalty_stamp` (demo burgers, and existing products in a
    "Burger" category on migration) earn one each when the order is fully paid; the item
    given as a reward earns nothing;
  - points: per MAD paid (rounded down);
  - `sync_order` (idempotent, called from `orders::recompute` and `void`) earns on payment,
    takes back on refund, gives a used reward back on void/refund/removal;
  - **reward**: a fixed order discount ("Fidélité"): stamps = the most expensive eligible
    item; points = up to the amount due. No manager PIN (the customer earned it), audited.
    Blocked while another order discount is set (one order discount at a time);
  - till: phone field for takeaway and delivery with live lookup (name/address filled in,
    balance shown), loyalty strip with "Use the reward" on the order; receipts show the
    balance after the order; product editor has "Earns a loyalty stamp".
- **Backups** (`backup.rs`):
  - SQLite online backup to `backups/` next to the database, written to a temporary file,
    integrity-checked, then renamed; never overwrites;
  - daily automatic backup on the Host (checked at start and every 30 minutes) when
    `backup_daily` is on; automatic backups older than `backup_keep_days` are deleted;
  - API `backup.list/create/copy_out` (`settings.manage`), `backup.restore` (**owner only**);
  - restore: from `backups/` or a `.db` put in Downloads/BURGER-OS; refuses files that aren't
    a BURGER OS database, are damaged, come from a newer version, or contain triggers/views;
    makes a "before-restore" copy first, restores, migrates to the current schema, clears every
    session (everyone logs in again) and tells every screen to reload;
  - Settings → Backups card: list, back up now, copy to Downloads, restore with confirmation.
- **Security review** (see below) and an automated permission test.
- **Arabic / polish**: the app name in the top bar and small-caps labels no longer spread
  Arabic letters apart (monospace font and letter spacing removed in RTL).
- **Release builds**:
  - Windows installer rebuilt (`src-tauri/target/release/bundle/nsis/BURGER OS_0.1.0_x64-setup.exe`);
  - Android release signing: Gradle reads a git-ignored `keystore.properties`;
    `scripts/make-android-key.ps1` creates the key on the owner's PC (owner's own password);
    `scripts/build-android.ps1` copies the APK to `release/` with a clear name and verifies
    the signature, or marks it `-UNSIGNED` when there's no key.

### Security review
Checked:
- **Permissions**: every API method (read from `api.rs` itself, so future ones are included)
  refuses a kitchen user except the ~25 the kitchen needs, and refuses everyone without a
  session except the 5 public ones (new test `every_method_checks_permissions`).
- **SQL**: no query is built from user input; table/column names in `format!` come from
  fixed lists.
- **LAN**: every request needs a paired device token (stored hashed); pairing closes after
  5 wrong PINs; user logins lock after 5 wrong PINs; sessions expire when idle; tokens
  come from a cryptographic RNG.
- **Webview**: strict Content-Security-Policy; only the app's own IPC command is allowed
  (no filesystem, shell or HTTP plugins); no `innerHTML` / `eval`.
- **Files**: export and backup names are checked (no paths); restore validates the file.
- **CSV**: text that starts with = + - @ is quoted (no formula injection in Excel).
Fixed: a restored file could have carried SQL triggers that run later inside the shop's
database; files with triggers or views are now refused (BURGER OS uses none).
Accepted, by design: LAN traffic is plain HTTP inside the shop network; any logged-in user
can see the print queue and retry a failed print; a cashier can look a customer up by phone
(needed for delivery); backups contain PIN hashes (Argon2), so keep backup files private.

### Verified
- **Rust: 126 tests pass** (8 new): phone normalisation and points rounding; customers found
  by phone and kept up to date, duplicates refused, search; stamps earned on payment, reward
  = most expensive burger, given back when removed, on refund and on void, idempotent;
  points capped by balance and amount due, manual adjustment never below zero, loyalty off,
  blocked by another discount; backup names, pruning, daily once, restore round trip
  (and back), damaged/foreign/trigger files refused, failed restores change nothing;
  backup permissions (owner-only restore, sessions cleared, topic "all"); every method
  checks permissions.
- **Vitest: 108 pass** (new: loyalty helpers; receipt shows the reward and balance).
  `tsc` clean.
- **The real app, driven by a script, on a throwaway database**: 22 checks pass, no page
  errors:
  - till: typing "+212 612345678" finds Sara (saved as "06 12 34 56 78"), fills her name,
    shows "Reward ready"; after adding a burger and sending, "Use the reward" makes it free
    (total 0.00, 3 stamps used);
  - Clients: list with balance; history shows earned and used; +2 stamps correction;
    invalid phone refused with a message;
  - backups: back up now, copy to Downloads (file exists), change a setting, restore →
    login screen, setting back to its old value, a "before restore" copy exists; a manager
    is refused;
  - Arabic: 18 apps visited right-to-left, no untranslated text, no page errors.
- Android release build without a key: BUILD SUCCESSFUL with the new signing config; with no key it produced `release/burger-os-0.1.0-arm64-v8a-UNSIGNED.apk` (10.3 MB) and warned, as intended. Not installed on a device.

### Not verified / known issues
- **Nothing has run on the tablet or with real printers yet** (see TODO.md, section 2).
- The signing key isn't created: the owner runs `scripts/make-android-key.ps1` (it's a
  credential only the owner should hold). Until then release APKs are unsigned.
- The Windows installer isn't code-signed (SmartScreen warning on install).
- A customer is only linked when the order is created with a phone number; adding a phone
  to an existing order doesn't link it.
- Only one order discount at a time: a manager discount and a loyalty reward can't be
  combined on the same order.
- The daily backup runs only while the Host app is open.
- The network test (`lan_test`) is timing-sensitive: once in this phase it timed out when two
  test runs overlapped on this PC; it passes on its own and in the normal full run.
- Your desktop app was not updated in this phase (see TODO.md).

## New look: "business" style (style B), 2026-11-23

The owner compared three mockups inspired by well-known ERPs (tile launchpad, side menu with
tables, apps with kanban) and chose **B**, the side menu with tables.

### Changed
- **Colours and font** (`src/styles.css`): light grey ground with white cards and a soft
  shadow, dark navy top bar, one blue accent (#0f6cbd), small corners. Windows' own font
  (Segoe UI; Roboto on Android). Numbers keep equal-width digits but no longer use the mono
  font. A dark theme with the same layout. IBM Plex stays bundled (the PDF export uses it).
- **Top bar**: home button, "BURGER OS | app name", search, register, connection, clock, a
  user button (initials, name, role; opens My profile) and lock.
- **Side menu** on every screen, grouped Sales / Menu and stock / Analysis / Administration.
  It folds into a rail of icons (button at the top; the choice is remembered on the device)
  and folds by itself on the till, kitchen and floor screens, which need the width. It is on
  the right in Arabic.
- **Home**: the same groups as tiles.
- **Shared parts** (`ui.tsx`): buttons (white with a grey border, or blue), inputs (white,
  darker bottom edge, blue underline when focused), switches, cards, dialogs, tabs
  (underlined), messages. Every table gets the same header, row lines and hover.
- Reports: the hour chart uses the accent colour.

### Verified
- `tsc` clean, Vitest 108 pass, production build OK. No Rust changes.
- Looked at it in the real app on the demo database (light, dark, Arabic): home, till (floor
  and takeaway order), stock, menu, clients, reports, settings. No page errors.

### Not verified
- Not seen on the tablet yet (side menu width on a small screen).
- New users still start in the dark theme (unchanged); each person can switch in My profile.

## Till in the "business" style (2026-11-23)

Mocked up first (canvas "Caisse · style B"), then built.

### Changed (`PosScreen.tsx`)
- **Two areas instead of three.** The right-hand pay column is gone, which gives the
  products more room. The amount to pay, **Envoyer en cuisine** and a large **Encaisser**
  are at the bottom of the order.
- **Command bar above the products** instead of the dark bar at the bottom: Orders (with
  the count of orders ready), Tables, New order | Discount, Hold, Split, Receipt | Void or
  Refund (red). Under 1440 px wide, Discount, Hold, Split and Receipt show as icons with a
  tooltip so the bar stays on one line.
- **Products are cards**: full name (two lines), price ("from …" when there are sizes),
  "Options" when a choice opens, a category-coloured top edge, greyed when out of stock.
- **Categories are underlined tabs** with a coloured dot, plus a **product search** that
  looks in every category (accents ignored).
- **Order lines**: tapping a line selects it (blue mark, on the right in Arabic). −, +,
  Note and Remove (or Void for a line already sent, which asks for a reason) act on it, for
  new items and for unsent saved lines alike. The quantity buttons are no longer inside
  every row.
- The order type is one segmented control; the customer and loyalty strips sit under it.

### Verified
- `tsc` clean, Vitest 108 pass.
- The real app on the demo database, driven by a script (8 checks, all pass, no page
  errors): customer found by phone, burger cards, search finds Coca-Cola from another
  category, line actions appear, + raises the quantity, send to kitchen, loyalty strip,
  reward used (total 74,00 → 24,00 DH), a sent line offers Void, Pay opens the payment.
  Looked at it in light, dark and Arabic.

### Not verified
- Not tried on the tablet's screen size yet.

## Kitchen screen in the "business" style (2026-11-23)

Mocked up first (canvas "Cuisine · style B"), then built.

### Changed (`KitchenScreen.tsx`)
- **Tickets are cards in columns** (oldest first) instead of table rows.
- **Card header coloured by waiting time**: light blue "On time", yellow "Almost late",
  red "LATE" with a red outline. It shows the big order number, type, table or customer,
  and the timer. The limits are the existing settings (`kds_warn_minutes`,
  `kds_late_minutes`).
- **Items have a large checkbox**; a ticked item is crossed out. Notes stay in a yellow
  box. The **Terminé** button is full width and shows progress ("Terminé · 1/2"). With
  "All stations" and several stations on a ticket, there is one button per station, as
  before.
- **Station tabs are underlined, with the number of tickets for each.** The screen now loads
  every open ticket once and filters by station itself (same result as the per-station
  query, since bumped items are already left out); this is what gives the counts.
- **Summary chips**: how many tickets are on time, almost late, late. Recall sits next to
  them.
- **New "To prepare" panel**: everything not done yet on the tickets shown, added up by
  product, so the grill can cook in batches.
- The new-ticket beep now also sounds for the first ticket on an empty screen (before,
  it only sounded once a ticket had been seen), and still not on first load or on
  switching station.
- Unchanged by design: ticking the last item of an order makes the order "ready", which
  takes it off the board (existing rule).

### Verified
- `tsc` clean, Vitest 108 pass.
- The real app on the demo database, driven by a script, light and dark (11 checks, all
  pass, no page errors): tickets as cards, "To prepare" totals, counts on the tabs, the note
  on the ticket, ticking an item, Terminé removes the ticket, the Grill tab hides fryer
  items and shows "1/1", recall brings the ticket back.

### Not verified
- Not tried on the kitchen tablet or from across a real kitchen.
