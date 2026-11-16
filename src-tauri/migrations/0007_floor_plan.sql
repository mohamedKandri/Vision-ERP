-- Floor plan: rooms (floors) and where each table sits on them.
-- Positions are grid cells on a 24 x 16 board; the screen scales the board to fit.
CREATE TABLE floors (
  id     INTEGER PRIMARY KEY,
  name   TEXT NOT NULL,
  sort   INTEGER NOT NULL DEFAULT 0,
  active INTEGER NOT NULL DEFAULT 1 CHECK (active IN (0, 1))
);
INSERT INTO floors (id, name, sort) VALUES (1, 'Salle', 1), (2, 'Terrasse', 2);

ALTER TABLE dining_tables ADD COLUMN floor_id INTEGER REFERENCES floors(id);
ALTER TABLE dining_tables ADD COLUMN x INTEGER NOT NULL DEFAULT 0 CHECK (x BETWEEN 0 AND 23);
ALTER TABLE dining_tables ADD COLUMN y INTEGER NOT NULL DEFAULT 0 CHECK (y BETWEEN 0 AND 15);
ALTER TABLE dining_tables ADD COLUMN w INTEGER NOT NULL DEFAULT 3 CHECK (w BETWEEN 1 AND 12);
ALTER TABLE dining_tables ADD COLUMN h INTEGER NOT NULL DEFAULT 3 CHECK (h BETWEEN 1 AND 12);
ALTER TABLE dining_tables ADD COLUMN shape TEXT NOT NULL DEFAULT 'square' CHECK (shape IN ('square', 'round'));

-- Lay out existing tables in rows of 5 so nothing overlaps; "Terrasse…" tables go on the terrace.
UPDATE dining_tables SET floor_id = CASE WHEN name LIKE 'Terrasse%' THEN 2 ELSE 1 END;
UPDATE dining_tables SET
  x = 1 + 4 * ((SELECT COUNT(*) FROM dining_tables d WHERE d.floor_id = dining_tables.floor_id
                AND (d.sort < dining_tables.sort OR (d.sort = dining_tables.sort AND d.id < dining_tables.id))) % 5),
  y = 1 + 4 * ((SELECT COUNT(*) FROM dining_tables d WHERE d.floor_id = dining_tables.floor_id
                AND (d.sort < dining_tables.sort OR (d.sort = dining_tables.sort AND d.id < dining_tables.id))) / 5 % 3),
  shape = CASE WHEN seats <= 2 THEN 'round' ELSE 'square' END;
CREATE INDEX orders_table_open ON orders(table_id) WHERE status NOT IN ('closed', 'voided');
