-- 0006: printers can be an HTTP endpoint (print bridge / cloud printer), and say how the
-- ESC/POS bytes are delivered: raw bytes, or base64 inside a JSON body.
-- SQLite can't change a CHECK constraint in place, so the table is rebuilt (foreign keys
-- are off during migrations and checked before commit; ids are kept, so routing survives).

CREATE TABLE printers_new (
  id               INTEGER PRIMARY KEY,
  name             TEXT NOT NULL CHECK (length(trim(name)) > 0),
  kind             TEXT NOT NULL CHECK (kind IN ('network', 'bluetooth', 'usb', 'sunmi', 'http')),
  address          TEXT NOT NULL DEFAULT '',
  paper_mm         INTEGER NOT NULL DEFAULT 80 CHECK (paper_mm IN (58, 80)),
  codepage         TEXT NOT NULL DEFAULT 'cp850' CHECK (codepage IN ('cp437', 'cp850', 'cp858', 'cp1252', 'ascii')),
  graphic_mode     INTEGER NOT NULL DEFAULT 0 CHECK (graphic_mode IN (0, 1)),
  cut              INTEGER NOT NULL DEFAULT 1 CHECK (cut IN (0, 1)),
  drawer           INTEGER NOT NULL DEFAULT 0 CHECK (drawer IN (0, 1)),
  receipt_register TEXT,
  ticket_lang      TEXT NOT NULL DEFAULT 'fr' CHECK (ticket_lang IN ('fr', 'en')),
  active           INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  last_ok_at       TEXT,
  last_error       TEXT,
  last_error_at    TEXT,
  -- raw: ESC/POS bytes as they are; base64: ESC/POS base64-encoded in a JSON body (HTTP only).
  format           TEXT NOT NULL DEFAULT 'raw' CHECK (format IN ('raw', 'base64')),
  -- HTTP only: JSON field that carries the base64 data, and an optional Authorization header.
  http_field       TEXT NOT NULL DEFAULT 'data',
  http_auth        TEXT NOT NULL DEFAULT ''
);

INSERT INTO printers_new (id, name, kind, address, paper_mm, codepage, graphic_mode, cut, drawer, receipt_register,
                          ticket_lang, active, last_ok_at, last_error, last_error_at)
SELECT id, name, kind, address, paper_mm, codepage, graphic_mode, cut, drawer, receipt_register,
       ticket_lang, active, last_ok_at, last_error, last_error_at
FROM printers;

DROP TABLE printers;
ALTER TABLE printers_new RENAME TO printers;
CREATE UNIQUE INDEX printers_one_receipt_per_register ON printers(receipt_register) WHERE receipt_register IS NOT NULL AND active = 1;
