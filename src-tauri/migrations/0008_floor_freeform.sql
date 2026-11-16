-- Free-form floor plan: 4x finer board (96 x 64 units), any size, rotation, and room elements
-- (walls, bar counter, door, window, plant, zone, label) so the plan looks like the real shop.
CREATE TABLE dining_tables_new (
  id       INTEGER PRIMARY KEY,
  name     TEXT NOT NULL,
  seats    INTEGER NOT NULL DEFAULT 4 CHECK (seats >= 1),
  sort     INTEGER NOT NULL DEFAULT 0,
  active   INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1)),
  floor_id INTEGER REFERENCES floors(id),
  x        INTEGER NOT NULL DEFAULT 0 CHECK (x BETWEEN 0 AND 95),
  y        INTEGER NOT NULL DEFAULT 0 CHECK (y BETWEEN 0 AND 63),
  w        INTEGER NOT NULL DEFAULT 12 CHECK (w BETWEEN 2 AND 96),
  h        INTEGER NOT NULL DEFAULT 12 CHECK (h BETWEEN 2 AND 64),
  shape    TEXT NOT NULL DEFAULT 'square' CHECK (shape IN ('square', 'round')),
  rotation INTEGER NOT NULL DEFAULT 0 CHECK (rotation BETWEEN 0 AND 359)
);
INSERT INTO dining_tables_new (id, name, seats, sort, active, floor_id, x, y, w, h, shape)
  SELECT id, name, seats, sort, active, floor_id, x * 4, y * 4, w * 4, h * 4, shape FROM dining_tables;
DROP TABLE dining_tables;
ALTER TABLE dining_tables_new RENAME TO dining_tables;

CREATE TABLE floor_items (
  id       INTEGER PRIMARY KEY,
  floor_id INTEGER NOT NULL REFERENCES floors(id) ON DELETE CASCADE,
  kind     TEXT NOT NULL CHECK (kind IN ('wall', 'bar', 'door', 'window', 'plant', 'zone', 'label')),
  label    TEXT NOT NULL DEFAULT '',
  color    TEXT NOT NULL DEFAULT '',
  x        INTEGER NOT NULL DEFAULT 0 CHECK (x BETWEEN 0 AND 95),
  y        INTEGER NOT NULL DEFAULT 0 CHECK (y BETWEEN 0 AND 63),
  w        INTEGER NOT NULL DEFAULT 8 CHECK (w BETWEEN 1 AND 96),
  h        INTEGER NOT NULL DEFAULT 8 CHECK (h BETWEEN 1 AND 64),
  rotation INTEGER NOT NULL DEFAULT 0 CHECK (rotation BETWEEN 0 AND 359),
  z        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX floor_items_floor ON floor_items(floor_id);
