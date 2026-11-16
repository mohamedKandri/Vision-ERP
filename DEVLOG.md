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
