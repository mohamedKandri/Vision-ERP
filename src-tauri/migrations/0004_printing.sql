-- 0004_printing: printers, routing, and a durable print queue.

CREATE TABLE printers (
  id               INTEGER PRIMARY KEY,
  name             TEXT NOT NULL CHECK (length(trim(name)) > 0),
  -- network: "host:port" | bluetooth: MAC or paired name | usb: Windows printer name | sunmi: (empty, uses "InnerPrinter")
  kind             TEXT NOT NULL CHECK (kind IN ('network', 'bluetooth', 'usb', 'sunmi')),
  address          TEXT NOT NULL DEFAULT '',
  paper_mm         INTEGER NOT NULL DEFAULT 80 CHECK (paper_mm IN (58, 80)),
  codepage         TEXT NOT NULL DEFAULT 'cp850' CHECK (codepage IN ('cp437', 'cp850', 'cp858', 'cp1252', 'ascii')),
  graphic_mode     INTEGER NOT NULL DEFAULT 0 CHECK (graphic_mode IN (0, 1)), -- print receipts as images
  cut              INTEGER NOT NULL DEFAULT 1 CHECK (cut IN (0, 1)),
  drawer           INTEGER NOT NULL DEFAULT 0 CHECK (drawer IN (0, 1)),       -- cash drawer wired to this printer
  receipt_register TEXT,                                                      -- prints customer receipts for this register
  ticket_lang      TEXT NOT NULL DEFAULT 'fr' CHECK (ticket_lang IN ('fr', 'en')),
  active           INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  last_ok_at       TEXT,
  last_error       TEXT,
  last_error_at    TEXT
);
CREATE UNIQUE INDEX printers_one_receipt_per_register ON printers(receipt_register) WHERE receipt_register IS NOT NULL AND active = 1;

-- A station can show on the kitchen screen, print, or both.
ALTER TABLE stations ADD COLUMN printer_id INTEGER REFERENCES printers(id);
ALTER TABLE stations ADD COLUMN show_on_screen INTEGER NOT NULL DEFAULT 1;

-- Durable queue. Kitchen tickets are written in the same transaction that sends the lines,
-- so a ticket exists even if the printer is off; the worker retries until it prints.
CREATE TABLE print_jobs (
  id              INTEGER PRIMARY KEY,
  printer_id      INTEGER NOT NULL REFERENCES printers(id),
  kind            TEXT NOT NULL CHECK (kind IN ('kitchen', 'receipt', 'report', 'test', 'drawer')),
  order_id        INTEGER REFERENCES orders(id),
  station_id      INTEGER REFERENCES stations(id),
  title           TEXT NOT NULL DEFAULT '',
  doc             TEXT NOT NULL, -- JSON document (lines) encoded to ESC/POS at print time
  status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'done', 'failed', 'cancelled')),
  attempts        INTEGER NOT NULL DEFAULT 0,
  last_error      TEXT,
  next_attempt_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  created_by      INTEGER REFERENCES users(id),
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  printed_at      TEXT
);
CREATE INDEX print_jobs_queue ON print_jobs(status, printer_id, id);
CREATE INDEX print_jobs_order ON print_jobs(order_id);
