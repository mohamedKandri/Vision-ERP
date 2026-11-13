-- 0005: paired devices (Host side), kitchen display parts, offline outbox (Client side).

-- Devices paired with this Host. Only a SHA-256 of the device token is stored.
CREATE TABLE devices (
  id         INTEGER PRIMARY KEY,
  name       TEXT NOT NULL,
  kind       TEXT NOT NULL CHECK (kind IN ('pos', 'kds', 'display', 'office')),
  token_hash TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  last_seen  TEXT,
  last_ip    TEXT,
  revoked    INTEGER NOT NULL DEFAULT 0 CHECK (revoked IN (0, 1))
);

-- One row per item per station on the kitchen screen. A combo line becomes several parts
-- (burger at the grill, fries at the fryer...). Created when lines are sent to the kitchen.
CREATE TABLE kitchen_parts (
  id         INTEGER PRIMARY KEY,
  order_id   INTEGER NOT NULL REFERENCES orders(id),
  line_id    INTEGER NOT NULL REFERENCES order_lines(id),
  part_index INTEGER NOT NULL DEFAULT 0,
  station_id INTEGER NOT NULL REFERENCES stations(id),
  qty        INTEGER NOT NULL,
  name       TEXT NOT NULL,   -- JSON names
  details    TEXT NOT NULL,   -- JSON {"fr":[..],"en":[..],"ar":[..]}
  note       TEXT NOT NULL DEFAULT '',
  sent_at    TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  done_at    TEXT,
  bumped_at  TEXT,            -- set when the station bumped the whole ticket (for recall)
  voided     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX kitchen_parts_station ON kitchen_parts(station_id, bumped_at, order_id);
CREATE INDEX kitchen_parts_order ON kitchen_parts(order_id);

-- Client devices: actions waiting for the Host while Wi-Fi is down.
CREATE TABLE outbox (
  id         INTEGER PRIMARY KEY,
  method     TEXT NOT NULL,
  params     TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  attempts   INTEGER NOT NULL DEFAULT 0,
  last_error TEXT
);
