-- 0010_purchasing: suppliers, what they sell, purchase orders, goods receipts.
-- Purchase prices are excluding VAT. Quantities are milli-units of the ingredient's unit.

CREATE TABLE suppliers (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL CHECK (length(trim(name)) > 0),
  phone      TEXT NOT NULL DEFAULT '',
  email      TEXT NOT NULL DEFAULT '',
  address    TEXT NOT NULL DEFAULT '',
  note       TEXT NOT NULL DEFAULT '',
  active     INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- An ingredient a supplier sells, by pack ("Carton 10 kg" = 10000000 g) at a pack price.
CREATE TABLE supplier_items (
  supplier_id     INTEGER NOT NULL REFERENCES suppliers(id) ON DELETE CASCADE,
  ingredient_id   INTEGER NOT NULL REFERENCES ingredients(id),
  pack_label      TEXT NOT NULL DEFAULT '',
  pack_milli      INTEGER NOT NULL CHECK (pack_milli > 0),
  pack_cost_cents INTEGER NOT NULL DEFAULT 0 CHECK (pack_cost_cents >= 0), -- last known price
  preferred       INTEGER NOT NULL DEFAULT 0 CHECK (preferred IN (0, 1)),
  PRIMARY KEY (supplier_id, ingredient_id)
);
-- One preferred supplier per ingredient.
CREATE UNIQUE INDEX supplier_items_preferred ON supplier_items(ingredient_id) WHERE preferred = 1;

CREATE TABLE purchase_orders (
  id            INTEGER PRIMARY KEY,
  number        INTEGER NOT NULL UNIQUE,
  supplier_id   INTEGER NOT NULL REFERENCES suppliers(id),
  status        TEXT NOT NULL DEFAULT 'draft' CHECK (status IN ('draft', 'ordered', 'partial', 'closed', 'cancelled')),
  note          TEXT NOT NULL DEFAULT '',
  expected_date TEXT NOT NULL DEFAULT '', -- YYYY-MM-DD or empty
  created_by    INTEGER NOT NULL REFERENCES users(id),
  created_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  ordered_at    TEXT,
  closed_at     TEXT
);
CREATE INDEX purchase_orders_status ON purchase_orders(status, created_at);

CREATE TABLE purchase_order_lines (
  id              INTEGER PRIMARY KEY,
  po_id           INTEGER NOT NULL REFERENCES purchase_orders(id) ON DELETE CASCADE,
  ingredient_id   INTEGER NOT NULL REFERENCES ingredients(id),
  pack_label      TEXT NOT NULL DEFAULT '',
  pack_milli      INTEGER NOT NULL CHECK (pack_milli > 0),
  qty_packs       INTEGER NOT NULL CHECK (qty_packs > 0),
  pack_cost_cents INTEGER NOT NULL CHECK (pack_cost_cents >= 0),
  received_milli  INTEGER NOT NULL DEFAULT 0 CHECK (received_milli >= 0),
  received_cents  INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX purchase_order_lines_po ON purchase_order_lines(po_id);

CREATE TABLE purchase_receipts (
  id      INTEGER PRIMARY KEY,
  po_id   INTEGER NOT NULL REFERENCES purchase_orders(id),
  note    TEXT NOT NULL DEFAULT '',
  user_id INTEGER NOT NULL REFERENCES users(id),
  at      TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TABLE purchase_receipt_lines (
  receipt_id  INTEGER NOT NULL REFERENCES purchase_receipts(id) ON DELETE CASCADE,
  po_line_id  INTEGER NOT NULL REFERENCES purchase_order_lines(id),
  qty_milli   INTEGER NOT NULL CHECK (qty_milli > 0),
  value_cents INTEGER NOT NULL CHECK (value_cents >= 0),
  PRIMARY KEY (receipt_id, po_line_id)
);

ALTER TABLE stock_moves ADD COLUMN po_id INTEGER REFERENCES purchase_orders(id);

INSERT INTO sequences (name, next_value) VALUES ('po_number', 1);
