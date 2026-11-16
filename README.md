# BURGER OS

A local-first POS + back office for one burger shop. It runs 100 % offline on Android
POS terminals/tablets and on a Windows PC. There's no cloud, no account and no telemetry.

> **Status: Phase 5 of 9.** You get setup, PIN login, staff and roles, permissions,
> settings, taxes, the audit log, and the full menu (products, sizes, modifiers, combos,
> out-of-stock board, happy hour), the till with payments, cash sessions (X/Z), and printing (receipts,
> kitchen tickets, cash drawer), several devices on the Wi-Fi, the kitchen screen and the
> customer screen. Stock, purchasing, reports and customers come in later phases (see [DEVLOG.md](DEVLOG.md)).

## Install

### Windows (back-office PC)
1. Run `BURGER OS_0.1.0_x64-setup.exe` (from `src-tauri/target/release/bundle/nsis/`).
2. Start **BURGER OS** from the Start menu.

The database is stored at `%APPDATA%\ma.burgeros.app\burger-os.db`.

### Android (POS terminal / tablet)
1. Copy the APK to the device, or install it over USB:
   ```bash
   adb install -r app-universal-release.apk
   ```
2. If you copy the file instead, allow "Install unknown apps" for your file manager once.
3. Open **BURGER OS**.

## First start

The first device you set up becomes the **Host**. It owns the database.

1. Choose a language, then enter the shop name, the owner's name and a 4–6 digit owner PIN.
2. Optionally create the demo staff: Karim (Manager, `2222`), Salma (Cashier, `3333`) and
   Youssef (Kitchen, `4444`). Change or deactivate them before going live.
3. On the login screen, tap your name and enter your PIN.

### Security notes
- After 5 wrong PINs, that user is locked for 60 seconds.
- The screen locks automatically after 5 minutes without a touch. Change this in **Settings → Sales**.
- Every login, failed login, manager override, setting change, tax change, staff change
  and permission change is written to the **Audit log**.

## Menu

**Menu** (Owner and Manager) has three tabs:
- **Products**: tap a product to edit it, or use 👁 to preview the popup the till will show.
  - Variants (Single/Double/Triple) each have their own price.
  - **Modifier groups** set what the cashier can pick: doneness, extras, remove…
  - A **combo** is made of slots (Burger / Side / Drink). Each choice can cost extra, and one is the default.
- **Categories**: color, icon and order.
- **Modifiers**: min/max rules, and option types *Choice*, *Extra* (+price, can repeat) and *Remove* (free).

**Ruptures** (also open to cashiers and the kitchen) marks an item out of stock in one tap.
The till won't sell it until it's back.

Happy hour: set a happy-hour price on a product or variant, then switch it on and set the
hours in **Settings → Happy hour**.

## Till (Caisse)

1. **Open the till** with its float (cashier or manager). The till takes no payment while closed.
2. Choose **Sur place** (pick a table), **À emporter** (customer name) or **Livraison**
   (phone + address; the delivery fee comes from Settings).
3. Tap products. Items with sizes, options or combo steps open a popup. Tap a cart line to
   add a note ("allergie sésame").
4. Use the chef-hat button to send to the kitchen, or **Encaisser** to send and take payment
   (cash with change, card, or both).
- **Discounts and comps:** every discount needs a manager PIN by default. Set the threshold in
  Settings → Sales. A comp always needs a reason.
- **Removing an item** already sent to the kitchen, **voiding**, **refunding** and **reopening**
  an order all need a manager (or their PIN) and are written to the audit log.
- **En attente** parks an order under a label; reopen it from the orders list.
- **Diviser l'addition** moves items onto a new bill (#42-2), before any payment.

**Caisse du jour** shows the live X report. **Clôturer (Z)** asks for the counted cash, shows
the difference, and freezes a numbered Z report.

## Taxes (Odoo-style)

Each tax has a rate and a **Price includes tax (TTC)** flag:
- **TTC** (on): the product price is what the customer pays, and VAT is extracted from it.
  A 45.00 DH burger at 10 % shows 40.91 HT + 4.09 VAT.
- **HT** (off): VAT is added on top of the product price.

You choose the tax per product (Phase 2). Four taxes come preset: TVA 10 % TTC,
TVA 20 % TTC, TVA 10 % HT and Exonéré 0 %. **Settings → Sales → VAT rounding** picks
per-line rounding or global rounding per tax.

## Host vs Client, pairing

- The **Host** is the device that keeps the data (usually the back-office PC or the main till).
  It must stay on and on the same Wi-Fi as the others. Any device you set up first is a Host.
- Every other till, kitchen screen or customer screen is a **Client**.

**Pair a new device:**
1. On the Host, log in as Owner, open **Appareil**, then **Associer un appareil**. A 6-digit
   code and the Host address (e.g. `192.168.1.20:7420`) appear, valid for 10 minutes.
2. Install the app on the new device. On its first screen tap **"Cet appareil rejoint une
   boutique existante"**. If the device is already in use, go to Appareil →
   **Rejoindre un Hôte**.
3. Type the address and the code, give the device a name, and choose what it's used as:
   **Caisse**, **Écran cuisine**, **Écran client** or **Bureau**.
4. Staff log in with their usual PINs. A kitchen screen opens straight on the kitchen view;
   a customer screen shows the Preparing/Ready board.

On Windows, allow BURGER OS through the firewall when asked, so the tablets can reach it.
In Appareil on the Host, **Retirer** disconnects a lost or stolen device.

**When the Wi-Fi drops:**
- A red banner shows on the till. New orders are kept on the tablet and sent automatically
  when the connection is back. Payments wait for the Host.
- A kitchen screen just reconnects.

## Kitchen screen

- Choose a station (Grill, Friteuse…) or **Tous**. Each ticket shows the number, the type and
  table, the items and notes, and a timer that turns orange and then red.
- Tap an item when it's done. **Terminé** sends the ticket away for this station, and
  **Rappeler le dernier** brings back the last one.
- When every station is done, the order turns **Prête**: the till beeps, and the customer
  screen shows the number.

## Stock

**Stock** (managers; the kitchen can see levels and record waste):
- **Stock** tab: every ingredient with quantity, minimum, cost and value. Low ones get a badge
  and a banner. **Record waste** (a dropped burger or 500 g of expired meat, with a reason),
  **Adjust** (+ delivery, − correction) and **Edit** (name in 3 languages, unit g / ml / piece,
  cost per kg / L / piece, minimum, opening stock).
  On an existing shop with no ingredients yet, **Load demo ingredients and recipes** adds a
  full set for the demo menu.
- **Recipes & food cost**: each product and size with its price excl. VAT, recipe cost and
  margin (green ≥ 65 %, orange ≥ 50 %, red below). Tap one to edit its recipe. A size with
  its own recipe replaces the product's. Extras add to the recipe; removals take away
  ("No onion" = −20 g of onion).
- Stock goes out by itself when an order is **sent to the kitchen** (or paid without being
  sent). A void gives it back (setting **Restore stock on void**); a refund gives it back only
  if **Restore stock on refund** is on. Turn on **Auto mark products out of stock** in
  Settings → Sales to 86 products automatically when an ingredient runs out.
- **Stock counts**: start a count, type what you find (leave a line empty to skip it), check
  the differences, then **Validate**. Only the lines you typed are corrected.
- **Movements**: every change, with who, why, the order number and the value.

## Purchasing (Achats)

**Achats** (managers):
- **Suppliers**: name, phone, and what each supplier sells, by pack (for example "Sachet 5 kg"
  of beef at 450,00 excl. VAT). Tick **Preferred** for the supplier to suggest first.
- **To order**: every ingredient at or under its minimum, with the packs needed to get back
  to twice the minimum (minus what is already on order). Untick what you don't want, change
  the packs, then **Create purchase orders**: one draft per supplier.
- **Purchase orders**: create or edit a draft, then **Place order**. When the delivery
  arrives, tap **Receive goods**. Enter what really came (part of it is fine), change the pack
  price if the invoice differs, and add the delivery-note number. Stock goes up, and the
  ingredient's cost becomes the average of the stock on hand and the delivery.
  **Close** an order that won't be completed; **Cancel** one nothing was received for.
  Each delivery appears in Stock → Movements as "Réception · BC-n".

## Printers

**Imprimantes** (Owner): **Ajouter une imprimante**, choose the type, then **Test**.

| Type | Address | Works on |
|---|---|---|
| Réseau (Wi-Fi/Ethernet) | the printer's IP, e.g. `192.168.1.50` (port 9100 by default, or `ip:port`) | all devices |
| Bluetooth | pair the printer in Android settings first, then choose it in the list | Android |
| USB (Windows) | install the printer in Windows (any driver), then choose it in the list | Windows |
| SUNMI intégrée | nothing to enter: uses the terminal's built-in "InnerPrinter" | SUNMI Android terminals |

- Choose **58 mm or 80 mm** paper. Keep the **PC850** character set unless accents print
  wrong; then try PC858 or Windows-1252.
- Switch on **"Imprimante des tickets clients de cette caisse"** on the printer near the till.
  Switch on **"Tiroir-caisse"** if the cash drawer is plugged into it.
- **Postes de cuisine**: give each station (Grill, Friteuse, Boissons, Montage) a printer,
  the kitchen screen, or both. A combo prints each part at its own station.
- If a printer is off or out of paper, tickets wait in the queue and the till shows a red
  banner. Fix the printer and tap **Réessayer**. A manager can send the waiting tickets to
  another printer, or delete one.
- Arabic receipts and printers set to **mode graphique** print the receipt as an image.


## Development

Requirements: Node 20+, pnpm 9, Rust 1.80+, and for Android: Android SDK, NDK 30 and the Rust Android targets.

```bash
pnpm install
```
```bash
pnpm tauri dev
```
```bash
pnpm test
```
```bash
pnpm test:rust
```
```bash
pnpm tauri build
```
On a Windows PC without Developer Mode, build the APK with the helper script.
It works around a symlink restriction and a JDK socket issue, both explained in DEVLOG.md:
```bash
powershell -ExecutionPolicy Bypass -File scripts/build-android.ps1
```
To install it on a tablet connected over USB with USB debugging enabled:
```bash
adb install -r release/burger-os-0.1.0-arm64-test.apk
```

### Layout
- `src/lib/money.ts`: pure money/VAT/discount/allocation logic (integer cents), tested in `money.test.ts`.
- `src/i18n/`: French, Arabic (RTL) and English strings, with a test that all three have the same keys.
- `src-tauri/src/api.rs`: the single RPC dispatcher. The webview calls `rpc(method, params)`.
  In Phase 5 the LAN server calls the same function, so auth and permissions live in one place.
- `src-tauri/migrations/`: forward-only SQL migrations.
- `src-tauri/src/purchasing.rs` + `src/lib/purchasing.ts`: suppliers, purchase orders, goods receipts (weighted average cost), suggestions.
- `src-tauri/src/stock.rs` + `src/lib/stock.ts`: stock quantities (thousandths of a unit), recipes, food cost; automatic deduction from orders.
- Debug builds read `BURGER_OS_DATA_DIR` to use another data folder, so you can test on a throwaway database. Release builds ignore it.
