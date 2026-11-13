-- 0003_orders: tables, cash sessions, orders, lines, payments.
-- Totals stored here are always recomputed by the Host (pricing.rs); clients never set them.

CREATE TABLE dining_tables (
  id     INTEGER PRIMARY KEY,
  name   TEXT NOT NULL,
  seats  INTEGER NOT NULL DEFAULT 4 CHECK (seats >= 1),
  sort   INTEGER NOT NULL DEFAULT 0,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

CREATE TABLE cash_sessions (
  id             INTEGER PRIMARY KEY,
  register       TEXT NOT NULL,
  opened_by      INTEGER NOT NULL REFERENCES users(id),
  opened_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  float_cents    INTEGER NOT NULL CHECK (float_cents >= 0),
  closed_by      INTEGER REFERENCES users(id),
  closed_at      TEXT,
  counted_cents  INTEGER,
  expected_cents INTEGER,
  z_number       INTEGER UNIQUE,
  note           TEXT,
  z_report       TEXT -- JSON snapshot taken at close
);
CREATE UNIQUE INDEX cash_sessions_one_open ON cash_sessions(register) WHERE closed_at IS NULL;

CREATE TABLE cash_movements (
  id           INTEGER PRIMARY KEY,
  session_id   INTEGER NOT NULL REFERENCES cash_sessions(id),
  kind         TEXT NOT NULL CHECK (kind IN ('in', 'out')),
  amount_cents INTEGER NOT NULL CHECK (amount_cents > 0),
  reason       TEXT NOT NULL CHECK (length(trim(reason)) > 0),
  user_id      INTEGER NOT NULL REFERENCES users(id),
  at           TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE orders (
  id                 INTEGER PRIMARY KEY,
  number             INTEGER NOT NULL,
  split_index        INTEGER NOT NULL DEFAULT 0,
  parent_id          INTEGER REFERENCES orders(id),
  type               TEXT NOT NULL CHECK (type IN ('dine_in', 'takeaway', 'delivery')),
  table_id           INTEGER REFERENCES dining_tables(id),
  customer_name      TEXT NOT NULL DEFAULT '',
  customer_phone     TEXT NOT NULL DEFAULT '',
  customer_address   TEXT NOT NULL DEFAULT '',
  delivery_fee_cents INTEGER NOT NULL DEFAULT 0 CHECK (delivery_fee_cents >= 0),
  delivery_tax_id    INTEGER REFERENCES taxes(id),
  note               TEXT NOT NULL DEFAULT '',
  -- Kitchen / service flow. 'closed' = paid and served. 'voided' = cancelled before payment.
  status             TEXT NOT NULL DEFAULT 'open'
                     CHECK (status IN ('open', 'sent', 'preparing', 'ready', 'served', 'closed', 'voided')),
  payment_status     TEXT NOT NULL DEFAULT 'unpaid' CHECK (payment_status IN ('unpaid', 'partial', 'paid', 'refunded')),
  held               INTEGER NOT NULL DEFAULT 0 CHECK (held IN (0, 1)),
  hold_label         TEXT NOT NULL DEFAULT '',
  discount_kind      TEXT CHECK (discount_kind IN ('percent', 'fixed')),
  discount_value     INTEGER,
  discount_reason    TEXT,
  discount_by        INTEGER REFERENCES users(id),
  subtotal_cents     INTEGER NOT NULL DEFAULT 0,
  discount_cents     INTEGER NOT NULL DEFAULT 0,
  base_cents         INTEGER NOT NULL DEFAULT 0,
  tax_cents          INTEGER NOT NULL DEFAULT 0,
  total_cents        INTEGER NOT NULL DEFAULT 0,
  paid_cents         INTEGER NOT NULL DEFAULT 0,
  vat                TEXT NOT NULL DEFAULT '[]', -- JSON VAT buckets
  register           TEXT NOT NULL DEFAULT 'main',
  created_by         INTEGER NOT NULL REFERENCES users(id),
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  sent_at            TEXT,
  ready_at           TEXT,
  served_at          TEXT,
  paid_at            TEXT,
  closed_at          TEXT,
  voided_at          TEXT,
  voided_by          INTEGER REFERENCES users(id),
  void_reason        TEXT,
  refunded_at        TEXT,
  refunded_by        INTEGER REFERENCES users(id),
  refund_reason      TEXT
);
CREATE UNIQUE INDEX orders_number ON orders(number, split_index);
CREATE INDEX orders_status ON orders(status, created_at);
CREATE INDEX orders_created ON orders(created_at);

CREATE TABLE order_lines (
  id              INTEGER PRIMARY KEY,
  order_id        INTEGER NOT NULL REFERENCES orders(id),
  product_id      INTEGER NOT NULL REFERENCES products(id),
  variant_id      INTEGER REFERENCES product_variants(id),
  config          TEXT NOT NULL,  -- JSON ItemConfig as chosen at the till
  names           TEXT NOT NULL,  -- JSON names snapshot of the product
  details         TEXT NOT NULL,  -- JSON {"fr":[..],"ar":[..],"en":[..]} variant/modifier lines
  qty             INTEGER NOT NULL CHECK (qty > 0),
  unit_cents      INTEGER NOT NULL CHECK (unit_cents >= 0),
  happy_hour      INTEGER NOT NULL DEFAULT 0,
  tax_id          INTEGER NOT NULL REFERENCES taxes(id),
  tax_rate_bp     INTEGER NOT NULL,
  price_include   INTEGER NOT NULL,
  discount_kind   TEXT CHECK (discount_kind IN ('percent', 'fixed', 'comp')),
  discount_value  INTEGER,
  discount_reason TEXT,
  discount_by     INTEGER REFERENCES users(id),
  gross_cents     INTEGER NOT NULL DEFAULT 0,
  discount_cents  INTEGER NOT NULL DEFAULT 0,
  base_cents      INTEGER NOT NULL DEFAULT 0,
  tax_cents       INTEGER NOT NULL DEFAULT 0,
  total_cents     INTEGER NOT NULL DEFAULT 0,
  note            TEXT NOT NULL DEFAULT '',
  station_id      INTEGER REFERENCES stations(id),
  kitchen_status  TEXT NOT NULL DEFAULT 'new' CHECK (kitchen_status IN ('new', 'sent', 'done')),
  sent_at         TEXT,
  voided          INTEGER NOT NULL DEFAULT 0 CHECK (voided IN (0, 1)),
  void_reason     TEXT,
  voided_by       INTEGER REFERENCES users(id),
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX order_lines_order ON order_lines(order_id);

CREATE TABLE payments (
  id              INTEGER PRIMARY KEY,
  order_id        INTEGER NOT NULL REFERENCES orders(id),
  kind            TEXT NOT NULL CHECK (kind IN ('payment', 'refund')),
  method          TEXT NOT NULL CHECK (method IN ('cash', 'card')),
  amount_cents    INTEGER NOT NULL, -- negative for refunds
  tendered_cents  INTEGER,
  change_cents    INTEGER,
  cash_session_id INTEGER REFERENCES cash_sessions(id),
  user_id         INTEGER NOT NULL REFERENCES users(id),
  at              TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  CHECK ((kind = 'payment' AND amount_cents > 0) OR (kind = 'refund' AND amount_cents < 0))
);
CREATE INDEX payments_order ON payments(order_id);
CREATE INDEX payments_session ON payments(cash_session_id);

INSERT INTO sequences (name, next_value) VALUES ('z_number', 1);

INSERT INTO dining_tables (name, seats, sort) VALUES
  ('T1', 2, 1), ('T2', 2, 2), ('T3', 4, 3), ('T4', 4, 4), ('T5', 4, 5),
  ('T6', 4, 6), ('T7', 6, 7), ('T8', 6, 8), ('Terrasse 1', 4, 9), ('Terrasse 2', 4, 10);
