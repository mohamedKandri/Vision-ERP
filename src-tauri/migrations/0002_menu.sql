-- 0002_menu: kitchen stations, categories, products, variants, modifiers, combos.
-- Names are JSON objects {"fr": "...", "ar": "...", "en": "..."}; "fr" is required.
-- Prices are integer cents in the basis of the product's tax (TTC or HT).

CREATE TABLE stations (
  id     INTEGER PRIMARY KEY,
  code   TEXT NOT NULL UNIQUE,
  names  TEXT NOT NULL,
  sort   INTEGER NOT NULL DEFAULT 0,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

CREATE TABLE categories (
  id     INTEGER PRIMARY KEY,
  names  TEXT NOT NULL,
  color  TEXT NOT NULL DEFAULT '#f59e0b',
  icon   TEXT NOT NULL DEFAULT 'burger',
  sort   INTEGER NOT NULL DEFAULT 0,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);

CREATE TABLE products (
  id                     INTEGER PRIMARY KEY,
  category_id            INTEGER NOT NULL REFERENCES categories(id),
  kind                   TEXT NOT NULL DEFAULT 'simple' CHECK (kind IN ('simple', 'combo')),
  names                  TEXT NOT NULL,
  description            TEXT NOT NULL DEFAULT '{}',
  price_cents            INTEGER NOT NULL CHECK (price_cents >= 0),
  happy_hour_price_cents INTEGER CHECK (happy_hour_price_cents IS NULL OR happy_hour_price_cents >= 0),
  tax_id                 INTEGER NOT NULL REFERENCES taxes(id),
  station_id             INTEGER REFERENCES stations(id),
  photo                  TEXT NOT NULL DEFAULT '',
  active                 INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  available              INTEGER NOT NULL DEFAULT 1 CHECK (available IN (0, 1)), -- 0 = 86'd
  sort                   INTEGER NOT NULL DEFAULT 0,
  updated_at             TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);
CREATE INDEX products_category ON products(category_id, sort);

-- Single / Double / Triple. When a product has variants, the variant price replaces the product price.
CREATE TABLE product_variants (
  id                     INTEGER PRIMARY KEY,
  product_id             INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
  names                  TEXT NOT NULL,
  price_cents            INTEGER NOT NULL CHECK (price_cents >= 0),
  happy_hour_price_cents INTEGER CHECK (happy_hour_price_cents IS NULL OR happy_hour_price_cents >= 0),
  active                 INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  sort                   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE modifier_groups (
  id         INTEGER PRIMARY KEY,
  names      TEXT NOT NULL,
  min_select INTEGER NOT NULL DEFAULT 0 CHECK (min_select >= 0),
  max_select INTEGER NOT NULL DEFAULT 1 CHECK (max_select >= 1),
  active     INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  sort       INTEGER NOT NULL DEFAULT 0,
  CHECK (min_select <= max_select)
);

-- kind: 'choice' (e.g. doneness), 'extra' (+price, may repeat up to max_qty), 'remove' (no onion, free).
CREATE TABLE modifier_options (
  id          INTEGER PRIMARY KEY,
  group_id    INTEGER NOT NULL REFERENCES modifier_groups(id) ON DELETE CASCADE,
  names       TEXT NOT NULL,
  kind        TEXT NOT NULL DEFAULT 'choice' CHECK (kind IN ('choice', 'extra', 'remove')),
  price_cents INTEGER NOT NULL DEFAULT 0 CHECK (price_cents >= 0),
  max_qty     INTEGER NOT NULL DEFAULT 1 CHECK (max_qty >= 1),
  is_default  INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
  active      INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  sort        INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE product_modifier_groups (
  product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
  group_id   INTEGER NOT NULL REFERENCES modifier_groups(id),
  sort       INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (product_id, group_id)
);

-- A combo (products.kind = 'combo') has slots: "Burger", "Side", "Drink".
CREATE TABLE combo_slots (
  id         INTEGER PRIMARY KEY,
  product_id INTEGER NOT NULL REFERENCES products(id) ON DELETE CASCADE,
  names      TEXT NOT NULL,
  sort       INTEGER NOT NULL DEFAULT 0
);

-- Choices for a slot. extra_cents is added to the combo price (e.g. premium burger +10).
CREATE TABLE combo_slot_options (
  id          INTEGER PRIMARY KEY,
  slot_id     INTEGER NOT NULL REFERENCES combo_slots(id) ON DELETE CASCADE,
  product_id  INTEGER NOT NULL REFERENCES products(id),
  variant_id  INTEGER REFERENCES product_variants(id),
  extra_cents INTEGER NOT NULL DEFAULT 0 CHECK (extra_cents >= 0),
  is_default  INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
  sort        INTEGER NOT NULL DEFAULT 0
);

INSERT INTO stations (code, names, sort) VALUES
  ('grill',    '{"fr":"Grill","ar":"الشواية","en":"Grill"}', 1),
  ('fryer',    '{"fr":"Friteuse","ar":"المقلاة","en":"Fryer"}', 2),
  ('drinks',   '{"fr":"Boissons","ar":"المشروبات","en":"Drinks"}', 3),
  ('assembly', '{"fr":"Montage","ar":"التجميع","en":"Assembly"}', 4);
