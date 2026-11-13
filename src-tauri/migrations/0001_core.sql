-- 0001_core: settings, users, permissions, taxes, sequences, audit log.
-- All money is stored as integer cents. Tax rates are basis points (1000 = 10.00%).
-- Timestamps are UTC ISO-8601 strings produced by SQLite.

CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL -- JSON-encoded value
);

CREATE TABLE users (
  id              INTEGER PRIMARY KEY,
  name            TEXT NOT NULL CHECK (length(trim(name)) > 0),
  role            TEXT NOT NULL CHECK (role IN ('owner', 'manager', 'cashier', 'kitchen')),
  pin_hash        TEXT NOT NULL,
  language        TEXT NOT NULL DEFAULT 'fr' CHECK (language IN ('fr', 'ar', 'en')),
  theme           TEXT NOT NULL DEFAULT 'dark' CHECK (theme IN ('dark', 'light')),
  active          INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  failed_attempts INTEGER NOT NULL DEFAULT 0,
  locked_until    TEXT,
  created_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- Owner always has every permission; it is never stored here.
CREATE TABLE role_permissions (
  role       TEXT NOT NULL CHECK (role IN ('manager', 'cashier', 'kitchen')),
  permission TEXT NOT NULL,
  PRIMARY KEY (role, permission)
);

-- Odoo-style taxes: price_include = 1 means product prices already contain the tax (TTC).
CREATE TABLE taxes (
  id            INTEGER PRIMARY KEY,
  name          TEXT NOT NULL,
  rate_bp       INTEGER NOT NULL CHECK (rate_bp >= 0 AND rate_bp <= 10000),
  price_include INTEGER NOT NULL DEFAULT 1 CHECK (price_include IN (0, 1)),
  active        INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

-- Monotonic counters (order numbers never reset).
CREATE TABLE sequences (
  name       TEXT PRIMARY KEY,
  next_value INTEGER NOT NULL
);

CREATE TABLE audit_log (
  id        INTEGER PRIMARY KEY,
  at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  user_id   INTEGER REFERENCES users(id),
  action    TEXT NOT NULL,
  entity    TEXT,
  entity_id TEXT,
  details   TEXT -- JSON
);
CREATE INDEX audit_log_at ON audit_log(at);

INSERT INTO taxes (name, rate_bp, price_include) VALUES
  ('TVA 10% TTC', 1000, 1),
  ('TVA 20% TTC', 2000, 1),
  ('TVA 10% HT',  1000, 0),
  ('Exonéré 0%',     0, 1);

INSERT INTO sequences (name, next_value) VALUES ('order_number', 1);

INSERT INTO role_permissions (role, permission) VALUES
  ('manager', 'pos.sell'), ('manager', 'pos.discount'), ('manager', 'pos.void'),
  ('manager', 'pos.refund'), ('manager', 'pos.reopen'), ('manager', 'cash.session'),
  ('manager', 'cash.in_out'), ('manager', 'drawer.open'), ('manager', 'kds.use'),
  ('manager', 'menu.manage'), ('manager', 'inventory.manage'), ('manager', 'purchasing.manage'),
  ('manager', 'reports.view'), ('manager', 'audit.view'), ('manager', 'customers.manage'),
  ('cashier', 'pos.sell'), ('cashier', 'cash.session'), ('cashier', 'cash.in_out'),
  ('cashier', 'drawer.open'), ('cashier', 'kds.use'),
  ('kitchen', 'kds.use');
