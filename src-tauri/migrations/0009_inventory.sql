-- 0009_inventory: ingredients, recipes, stock ledger, stock counts.
-- Quantities are integer milli-units of the ingredient's unit: 150 g = 150000, 1 piece = 1000.
-- cost_cents is per kg (unit g), per litre (unit ml) or per piece.

CREATE TABLE ingredients (
  id          INTEGER PRIMARY KEY,
  names       TEXT NOT NULL,
  unit        TEXT NOT NULL CHECK (unit IN ('g', 'ml', 'piece')),
  cost_cents  INTEGER NOT NULL DEFAULT 0 CHECK (cost_cents >= 0),
  stock_milli INTEGER NOT NULL DEFAULT 0,  -- cached SUM(stock_moves.delta_milli); may go negative
  min_milli   INTEGER NOT NULL DEFAULT 0 CHECK (min_milli >= 0),
  active      INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  sort        INTEGER NOT NULL DEFAULT 0,
  updated_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

-- What one unit of a product, a size (variant) or a modifier option consumes.
-- A size with its own lines replaces the product's lines ("Double" = 2 patties).
-- Option lines are added per option chosen; a negative quantity removes ("No onion").
CREATE TABLE recipe_lines (
  id            INTEGER PRIMARY KEY,
  product_id    INTEGER REFERENCES products(id) ON DELETE CASCADE,
  variant_id    INTEGER REFERENCES product_variants(id) ON DELETE CASCADE,
  option_id     INTEGER REFERENCES modifier_options(id) ON DELETE CASCADE,
  ingredient_id INTEGER NOT NULL REFERENCES ingredients(id),
  qty_milli     INTEGER NOT NULL CHECK (qty_milli <> 0),
  CHECK ((product_id IS NOT NULL) + (variant_id IS NOT NULL) + (option_id IS NOT NULL) = 1)
);
CREATE INDEX recipe_lines_product ON recipe_lines(product_id);
CREATE INDEX recipe_lines_variant ON recipe_lines(variant_id);
CREATE INDEX recipe_lines_option ON recipe_lines(option_id);
CREATE INDEX recipe_lines_ingredient ON recipe_lines(ingredient_id);

CREATE TABLE stock_counts (
  id         INTEGER PRIMARY KEY,
  status     TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'done', 'cancelled')),
  note       TEXT NOT NULL DEFAULT '',
  created_by INTEGER NOT NULL REFERENCES users(id),
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  closed_by  INTEGER REFERENCES users(id),
  closed_at  TEXT
);
CREATE UNIQUE INDEX stock_counts_one_open ON stock_counts(status) WHERE status = 'open';

CREATE TABLE stock_count_lines (
  count_id       INTEGER NOT NULL REFERENCES stock_counts(id) ON DELETE CASCADE,
  ingredient_id  INTEGER NOT NULL REFERENCES ingredients(id),
  counted_milli  INTEGER CHECK (counted_milli IS NULL OR counted_milli >= 0),
  expected_milli INTEGER, -- system stock at validation
  PRIMARY KEY (count_id, ingredient_id)
);

-- Every stock change. Sales/returns are tied to the order line (no FK: a line can be deleted
-- or moved to a split check; the ledger keeps the id).
CREATE TABLE stock_moves (
  id            INTEGER PRIMARY KEY,
  ingredient_id INTEGER NOT NULL REFERENCES ingredients(id),
  delta_milli   INTEGER NOT NULL CHECK (delta_milli <> 0),
  kind          TEXT NOT NULL CHECK (kind IN ('sale', 'return', 'waste', 'adjust', 'count', 'purchase', 'initial')),
  reason        TEXT NOT NULL DEFAULT '',
  value_cents   INTEGER NOT NULL DEFAULT 0, -- delta valued at the cost at that time (negative when stock goes out)
  order_id      INTEGER,
  order_line_id INTEGER,
  count_id      INTEGER REFERENCES stock_counts(id),
  product_id    INTEGER REFERENCES products(id), -- product wasted, when waste was entered per product
  user_id       INTEGER REFERENCES users(id),
  at            TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX stock_moves_ingredient ON stock_moves(ingredient_id, at);
CREATE INDEX stock_moves_line ON stock_moves(order_line_id);
CREATE INDEX stock_moves_order ON stock_moves(order_id);
CREATE INDEX stock_moves_at ON stock_moves(at);

-- Consumption of one unit of the line, frozen the first time the line uses stock, so a later
-- recipe change never rewrites past sales. JSON {"<ingredient_id>": qty_milli}.
ALTER TABLE order_lines ADD COLUMN stock_per_unit TEXT;

-- 1 = marked unavailable automatically because an ingredient ran out (cleared on restock
-- or when someone changes availability by hand).
ALTER TABLE products ADD COLUMN stock_out INTEGER NOT NULL DEFAULT 0 CHECK (stock_out IN (0, 1));
