# Vision: what's left

Status on 2026-11-23: all 9 phases are built and committed. This list comes from the spec
and from the "Not verified / known issues" notes in `DEVLOG.md`.

Legend: **[me]** I can do it here · **[you]** needs you (a device, a printer, a decision)

---

## 1. Phase 9 (from the spec): done

- [x] **Customers**: found by phone at the till (name and address filled in), Clients app,
      order history.
- [x] **Loyalty**: stamps or points (Settings), reward at the till, shown on the receipt.
- [x] **Backup and restore**: daily automatic backup, back up now, copy to Downloads,
      restore (owner only).
- [x] **Arabic check**: every app visited right to left; letter spacing fixed in the title
      bar and labels.
- [x] **Security review**: permission test over every API method; restore refuses files
      with hidden triggers.
- [x] **Windows installer** rebuilt.
- [ ] **Create the Android signing key** [you]: run
      `powershell -ExecutionPolicy Bypass -File scripts/make-android-key.ps1`, choose a password,
      then back up `%USERPROFILE%\BURGER-OS-signing` and the password. Then
      `scripts/build-android.ps1` makes the signed APK.
- [ ] **Code-signing certificate for the Windows installer** [you]: optional, paid; without
      it Windows SmartScreen warns on install.
- [x] **Desktop app updated** to the Phase 9 build with the new look (old one kept as `burger-os.old-phase8.exe`).

## 2. Testing on real hardware (all need you)

Nothing below has run on a real device yet.

- [ ] Install the APK on the tablet over USB and use it by touch. Check how long the PIN
      login takes on the tablet (it may need tuning).
- [ ] Printers:
  - [ ] network (Wi-Fi/Ethernet) receipt printer;
  - [ ] Bluetooth printer;
  - [ ] SUNMI built-in printer;
  - [ ] USB printer on Windows;
  - [ ] cash drawer opening.
  I need the models, or to test with you.
- [ ] Two devices on the shop Wi-Fi: PC or tablet as Host, a tablet as Client, plus the
      kitchen screen. Check the Windows firewall prompt and pairing.
- [ ] Export CSV/PDF on Android: check where the file lands.
- [ ] Android builds on this PC need a workaround script; turning on **Windows Developer
      Mode** removes one of the workarounds [you].

## 3. Known gaps: decide what to change

Each one is a choice; tick the ones you want and I'll add them.

**Till**
- [ ] Partial refunds (today: whole order only).
- [ ] Split a bill into equal amounts (today: by item, or card + cash).

**Printing and kitchen**
- [ ] Kitchen tickets in Arabic (today: French or English).
- [ ] A drawer setting for printers that only respond to the other drawer command.
- [ ] Auto-86 for combos and options (today: a combo stays on sale when its default
      burger runs out; the till still checks each choice).

**Devices and network**
- [ ] Scan the pairing QR with the tablet camera (today: type the address and PIN).
- [ ] Offline mode beyond new orders (today: payments and edits need the Host).
- [ ] Keep users logged in on the tills after the Host restarts.
- [ ] Encrypt LAN traffic (today: plain HTTP inside the shop network, with pairing tokens
      and PINs).

**Stock and purchasing**
- [ ] Print / PDF a purchase order to send to the supplier.
- [ ] Track purchase VAT (today: prices excl. VAT only).

**Reports**
- [ ] "Share" / "Open" button after an export.
- [ ] Freeze each order's totals at payment, so reopening an order doesn't change past
      reports.
- [ ] Selectable text in PDFs (today: page images, so Arabic renders correctly).

## 4. Your changes

Write here anything you saw in the app that you want changed (screens, wording, prices,
workflow):

- [ ] 
