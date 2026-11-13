# BURGER OS

A local-first POS + back office for one burger shop. It runs 100 % offline on Android
POS terminals/tablets and on a Windows PC. There's no cloud, no account and no telemetry.

> **Status: Phase 2 of 9.** You get setup, PIN login, staff and roles, permissions,
> settings, taxes, the audit log, and the full menu (products, sizes, modifiers, combos,
> out-of-stock board, happy hour). The till, kitchen screen, printers and LAN sync
> come in later phases (see [DEVLOG.md](DEVLOG.md)).

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

## Taxes (Odoo-style)

Each tax has a rate and a **Price includes tax (TTC)** flag:
- **TTC** (on): the product price is what the customer pays, and VAT is extracted from it.
  A 45.00 DH burger at 10 % shows 40.91 HT + 4.09 VAT.
- **HT** (off): VAT is added on top of the product price.

You choose the tax per product (Phase 2). Four taxes come preset: TVA 10 % TTC,
TVA 20 % TTC, TVA 10 % HT and Exonéré 0 %. **Settings → Sales → VAT rounding** picks
per-line rounding or global rounding per tax.

## Host vs Client, pairing, printers

These arrive in later phases. This section will cover them as they land:
- Phase 4: adding printers (network ESC/POS on port 9100, Bluetooth, USB, SUNMI).
- Phase 5: Host/Client mode, and pairing a terminal or kitchen screen with a QR code and PIN.

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
