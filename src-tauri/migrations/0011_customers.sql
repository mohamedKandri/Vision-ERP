-- 0011_customers: customer list (by phone), loyalty ledger, products that earn a stamp.

CREATE TABLE customers (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL DEFAULT '',
  phone      TEXT NOT NULL CHECK (length(phone) BETWEEN 6 AND 16), -- normalised: digits, Moroccan +212 as 0
  address    TEXT NOT NULL DEFAULT '',
  note       TEXT NOT NULL DEFAULT '',
  active     INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE UNIQUE INDEX customers_phone ON customers(phone);

ALTER TABLE orders ADD COLUMN customer_id INTEGER REFERENCES customers(id);
-- Reward used on this order (given back if the order is voided or refunded).
ALTER TABLE orders ADD COLUMN loyalty_redeem_stamps INTEGER NOT NULL DEFAULT 0;
ALTER TABLE orders ADD COLUMN loyalty_redeem_points INTEGER NOT NULL DEFAULT 0;
CREATE INDEX orders_customer ON orders(customer_id);

-- Products that earn a stamp ("every Nth burger free").
ALTER TABLE products ADD COLUMN loyalty_stamp INTEGER NOT NULL DEFAULT 0 CHECK (loyalty_stamp IN (0, 1));
UPDATE products SET loyalty_stamp = 1
 WHERE kind = 'simple'
   AND category_id IN (SELECT id FROM categories WHERE lower(json_extract(names, '$.fr')) LIKE '%burger%');

-- The ledger is the truth: a balance is the sum of its moves.
CREATE TABLE loyalty_moves (
  id          INTEGER PRIMARY KEY,
  customer_id INTEGER NOT NULL REFERENCES customers(id),
  order_id    INTEGER REFERENCES orders(id),
  kind        TEXT NOT NULL CHECK (kind IN ('earn', 'redeem', 'reverse', 'adjust')),
  stamps      INTEGER NOT NULL DEFAULT 0,
  points      INTEGER NOT NULL DEFAULT 0,
  reason      TEXT,
  user_id     INTEGER REFERENCES users(id),
  at          TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  CHECK (stamps <> 0 OR points <> 0)
);
CREATE INDEX loyalty_moves_customer ON loyalty_moves(customer_id, at);
CREATE INDEX loyalty_moves_order ON loyalty_moves(order_id);
