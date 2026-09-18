CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, name TEXT NOT NULL, applied_at TEXT NOT NULL);

      CREATE TABLE stations (
        id TEXT PRIMARY KEY,
        station_name TEXT NOT NULL,
        short_name TEXT NOT NULL,
        total_docks INTEGER NOT NULL,
        docks_in_service INTEGER NOT NULL,
        status TEXT NOT NULL,
        latitude REAL NOT NULL,
        longitude REAL NOT NULL,
        is_electric INTEGER NOT NULL CHECK (is_electric IN (0, 1)),
        state_hash TEXT NOT NULL,
        first_seen_at TEXT NOT NULL,
        last_seen_at TEXT NOT NULL
      );

      CREATE TABLE events (
        id TEXT PRIMARY KEY,
        dedupe_key TEXT NOT NULL UNIQUE,
        type TEXT NOT NULL CHECK (
          type IN ('station.discovered', 'station.electrified')
        ),
        station_id TEXT NOT NULL REFERENCES stations(id),
        payload TEXT NOT NULL,
        created_at TEXT NOT NULL
      );

      CREATE TABLE deliveries (
        id TEXT PRIMARY KEY,
        event_id TEXT NOT NULL UNIQUE REFERENCES events(id),
        record_key TEXT NOT NULL UNIQUE,
        status TEXT NOT NULL CHECK (
          status IN ('pending', 'processing', 'retrying', 'delivered')
        ),
        attempts INTEGER NOT NULL DEFAULT 0,
        next_attempt_at TEXT NOT NULL,
        locked_at TEXT,
        last_attempt_at TEXT,
        last_error TEXT,
        post_uri TEXT,
        post_cid TEXT,
        created_at TEXT NOT NULL,
        delivered_at TEXT
      );

      CREATE INDEX deliveries_ready_idx
        ON deliveries(status, next_attempt_at, created_at);

      CREATE TABLE runs (
        id TEXT PRIMARY KEY,
        started_at TEXT NOT NULL,
        finished_at TEXT,
        status TEXT NOT NULL CHECK (
          status IN ('running', 'succeeded', 'failed')
        ),
        stations_fetched INTEGER,
        events_created INTEGER,
        deliveries_completed INTEGER,
        error TEXT
      );
    

      ALTER TABLE deliveries
      ADD COLUMN announcement_style TEXT
      CHECK (announcement_style IN ('civic', 'nightline'));

      UPDATE deliveries
      SET announcement_style = CASE
        WHEN rowid % 2 = 1 THEN 'civic'
        ELSE 'nightline'
      END;
    
INSERT INTO schema_migrations VALUES(1,'initial durable station and delivery schema','2026-07-25T00:00:00.000Z'),(2,'persist alternating announcement styles','2026-07-25T00:00:00.000Z');
