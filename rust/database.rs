use crate::domain::{Delivery, EventType, Payload, Station, Style, encode_tid, now};
use anyhow::{Context, Result, ensure};
use chrono::{SecondsFormat, Utc};
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension, Row, params};
use serde::Serialize;
use std::{
    collections::HashSet,
    fs::{File, OpenOptions},
    path::Path,
    time::Duration,
};
use uuid::Uuid;

const SCHEMA: &str = include_str!("schema.sql");
pub struct Database {
    pub connection: Connection,
    _lock: Option<File>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reconciliation {
    pub baseline_created: bool,
    pub stations_processed: usize,
    pub discovered: usize,
    pub electrified: usize,
    pub events_created: usize,
}

impl Database {
    pub fn open_readonly(path: &Path) -> Result<Self> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .context("Open existing database read-only")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let db = Self {
            connection,
            _lock: None,
        };
        db.validate_schema()?;
        Ok(db)
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let lock_path = path.with_extension("sqlite3.run.lock");
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)?;
        lock.try_lock_exclusive()
            .context("Another worker owns this database; refusing overlapping run")?;
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let object_count: i64 = connection.query_row(
            "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if object_count == 0 {
            connection.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(error) = connection.execute_batch(SCHEMA) {
                connection.execute_batch("ROLLBACK")?;
                return Err(error.into());
            }
            connection.execute("UPDATE schema_migrations SET applied_at=?1", [now()])?;
            connection.execute_batch("COMMIT")?;
        }
        let db = Self {
            connection,
            _lock: Some(lock),
        };
        db.validate_schema()?;
        db.connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA cache_size=-2048;")?;
        // Existing databases retain their journal mode and all migration rows.
        if object_count == 0 {
            db.connection.pragma_update(None, "journal_mode", "WAL")?;
        }
        Ok(db)
    }
    pub fn validate_schema(&self) -> Result<()> {
        let versions = self
            .connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")?
            .query_map([], |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ensure!(
            versions == [1, 2],
            "Unsupported database schema: expected versions 1 and 2; no automatic migration is performed"
        );
        self.connection.prepare("SELECT id, station_name, short_name, total_docks, docks_in_service, status, latitude, longitude, is_electric, state_hash, first_seen_at, last_seen_at FROM stations LIMIT 0")?;
        self.connection.prepare("SELECT id, event_id, record_key, announcement_style, status, attempts, next_attempt_at, locked_at, last_attempt_at, last_error, post_uri, post_cid, created_at, delivered_at FROM deliveries LIMIT 0")?;
        self.connection.prepare(
            "SELECT id, dedupe_key, type, station_id, payload, created_at FROM events LIMIT 0",
        )?;
        self.connection.prepare("SELECT id, started_at, finished_at, status, stations_fetched, events_created, deliveries_completed, error FROM runs LIMIT 0")?;
        Ok(())
    }
    pub fn count(&self) -> Result<i64> {
        Ok(self
            .connection
            .query_row("SELECT count(*) FROM stations", [], |r| r.get(0))?)
    }
    pub fn station(&self, id: &str) -> Result<Option<Station>> {
        Ok(self.connection.query_row("SELECT id, station_name, short_name, total_docks, docks_in_service, status, latitude, longitude, is_electric FROM stations WHERE id=?", [id], station_row).optional()?)
    }
    pub fn status(&self) -> Result<serde_json::Value> {
        let mut counts = serde_json::json!({"pending":0,"processing":0,"retrying":0,"delivered":0});
        let mut statement = self
            .connection
            .prepare("SELECT status, count(*) FROM deliveries GROUP BY status")?;
        for row in statement.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
            let (status, count) = row?;
            counts[status] = count.into();
        }
        Ok(serde_json::json!({"stationCount":self.count()?,"deliveryCounts":counts}))
    }
    pub fn check(&self) -> Result<()> {
        let integrity: String = self
            .connection
            .query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        ensure!(integrity == "ok", "Database integrity check failed");
        ensure!(
            !self
                .connection
                .prepare("PRAGMA foreign_key_check")?
                .exists([])?,
            "Database has foreign-key violations"
        );
        let mut statement = self.connection.prepare("SELECT record_key, announcement_style, payload FROM deliveries JOIN events ON events.id=deliveries.event_id")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            crate::domain::reply_key(&row.get::<_, String>(0)?)?;
            let style: String = row.get(1)?;
            ensure!(
                matches!(style.as_str(), "civic" | "nightline"),
                "Invalid persisted style"
            );
            let payload: Payload = serde_json::from_str(&row.get::<_, String>(2)?)?;
            payload.station.validate()?;
        }
        Ok(())
    }
    pub fn reconcile(&mut self, incoming: &[Station], observed: &str) -> Result<Reconciliation> {
        ensure!(!incoming.is_empty(), "Refusing an empty station snapshot");
        let mut ids = HashSet::with_capacity(incoming.len());
        for station in incoming {
            station.validate()?;
            ensure!(ids.insert(&station.id), "Duplicate station ID");
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let baseline: bool = tx.query_row("SELECT count(*)=0 FROM stations", [], |r| r.get(0))?;
        let mut result = Reconciliation {
            baseline_created: baseline,
            stations_processed: incoming.len(),
            discovered: 0,
            electrified: 0,
            events_created: 0,
        };
        {
            let mut lookup = tx.prepare_cached("SELECT is_electric FROM stations WHERE id=?")?;
            let mut upsert=tx.prepare_cached("INSERT INTO stations(id,station_name,short_name,total_docks,docks_in_service,status,latitude,longitude,is_electric,state_hash,first_seen_at,last_seen_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?11) ON CONFLICT(id) DO UPDATE SET station_name=excluded.station_name,short_name=excluded.short_name,total_docks=excluded.total_docks,docks_in_service=excluded.docks_in_service,status=excluded.status,latitude=excluded.latitude,longitude=excluded.longitude,is_electric=excluded.is_electric,state_hash=excluded.state_hash,last_seen_at=excluded.last_seen_at")?;
            let mut next_tid = (Utc::now().timestamp_micros() as u64) << 10;
            for station in incoming {
                let electric: Option<bool> =
                    lookup.query_row([&station.id], |r| r.get(0)).optional()?;
                upsert.execute(params![
                    station.id,
                    station.station_name,
                    station.short_name,
                    station.total_docks,
                    station.docks_in_service,
                    station.status,
                    station.latitude,
                    station.longitude,
                    station.is_electric,
                    station.state_hash()?,
                    observed
                ])?;
                if baseline {
                    continue;
                }
                let kind = match electric {
                    None => {
                        result.discovered += 1;
                        EventType::Discovered
                    }
                    Some(false) if station.is_electric => {
                        result.electrified += 1;
                        EventType::Electrified
                    }
                    _ => continue,
                };
                let event_id = Uuid::new_v4().to_string();
                let payload =
                    serde_json::json!({"type":kind,"station":station,"observedAt":observed});
                let inserted=tx.execute("INSERT INTO events(id,dedupe_key,type,station_id,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(dedupe_key) DO NOTHING",params![event_id,format!("{}:{}",kind.as_str(),station.id),kind.as_str(),station.id,serde_json::to_string(&payload)?,observed])?;
                if inserted == 0 {
                    continue;
                }
                let previous: Option<String> = tx
                    .query_row(
                        "SELECT announcement_style FROM deliveries ORDER BY rowid DESC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?;
                let style = if previous.as_deref() == Some("civic") {
                    Style::Nightline
                } else {
                    Style::Civic
                };
                // Reserve separate timestamps for each event; reply clocks cannot collide with roots.
                next_tid += 1024;
                tx.execute("INSERT INTO deliveries(id,event_id,record_key,announcement_style,status,next_attempt_at,created_at) VALUES(?1,?2,?3,?4,'pending',?5,?5)",params![Uuid::new_v4().to_string(),event_id,encode_tid(next_tid),style.as_str(),observed])?;
                result.events_created += 1;
            }
        }
        tx.commit()?;
        Ok(result)
    }
    pub fn recover(&self) -> Result<usize> {
        let before = (Utc::now() - chrono::Duration::minutes(30))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        Ok(self.connection.execute("UPDATE deliveries SET status='retrying',locked_at=NULL,next_attempt_at=?1,last_error=coalesce(last_error,'Recovered abandoned delivery') WHERE status='processing' AND locked_at<?2",params![now(),before])?)
    }
    pub fn ready(&self, limit: usize) -> Result<Vec<Delivery>> {
        let mut statement=self.connection.prepare("SELECT deliveries.id,record_key,announcement_style,payload FROM deliveries JOIN events ON events.id=event_id WHERE deliveries.status IN ('pending','retrying') AND next_attempt_at<=?1 ORDER BY deliveries.rowid LIMIT ?2")?;
        let mut rows = statement.query(params![now(), limit as i64])?;
        let mut deliveries = Vec::new();
        while let Some(row) = rows.next()? {
            let style: String = row.get(2)?;
            deliveries.push(Delivery {
                id: row.get(0)?,
                record_key: row.get(1)?,
                style: match style.as_str() {
                    "civic" => Style::Civic,
                    "nightline" => Style::Nightline,
                    _ => anyhow::bail!("Invalid delivery style"),
                },
                payload: serde_json::from_str(&row.get::<_, String>(3)?)?,
            });
        }
        Ok(deliveries)
    }
    pub fn claim(&self, id: &str) -> Result<bool> {
        Ok(self.connection.execute("UPDATE deliveries SET status='processing',locked_at=?1,last_attempt_at=?1,attempts=attempts+1 WHERE id=?2 AND status IN ('pending','retrying') AND next_attempt_at<=?1",params![now(),id])?==1)
    }
    pub fn delivered(&self, id: &str, uri: &str, cid: &str) -> Result<()> {
        ensure!(self.connection.execute("UPDATE deliveries SET status='delivered',locked_at=NULL,last_error=NULL,post_uri=?1,post_cid=?2,delivered_at=?3 WHERE id=?4 AND status='processing'",params![uri,cid,now(),id])?==1,"Delivery claim lost");
        Ok(())
    }
    pub fn retry(&self, id: &str, message: &str) -> Result<()> {
        let attempts: u32 =
            self.connection
                .query_row("SELECT attempts FROM deliveries WHERE id=?", [id], |r| {
                    r.get(0)
                })?;
        let minutes = (1i64 << attempts.saturating_sub(1).min(10)).min(720);
        let next = (Utc::now() + chrono::Duration::minutes(minutes))
            .to_rfc3339_opts(SecondsFormat::Millis, true);
        self.connection.execute("UPDATE deliveries SET status='retrying',locked_at=NULL,last_error=?1,next_attempt_at=?2 WHERE id=?3 AND status='processing'",params![message,next,id])?;
        Ok(())
    }
    pub fn start_run(&self) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        self.connection.execute(
            "INSERT INTO runs(id,started_at,status) VALUES(?1,?2,'running')",
            params![id, now()],
        )?;
        Ok(id)
    }
    pub fn finish_run(
        &self,
        id: &str,
        fetched: usize,
        events: usize,
        delivered: usize,
        error: Option<&str>,
    ) -> Result<()> {
        self.connection.execute("UPDATE runs SET status=?1,finished_at=?2,stations_fetched=?3,events_created=?4,deliveries_completed=?5,error=?6 WHERE id=?7 AND status='running'",params![if error.is_some(){"failed"}else{"succeeded"},now(),fetched as i64,events as i64,delivered as i64,error,id])?;
        Ok(())
    }
}
pub fn station_row(r: &Row<'_>) -> rusqlite::Result<Station> {
    Ok(Station {
        id: r.get(0)?,
        station_name: r.get(1)?,
        short_name: r.get::<_, Option<String>>(2)?.unwrap_or_default(),
        total_docks: r.get(3)?,
        docks_in_service: r.get(4)?,
        status: r.get(5)?,
        latitude: r.get(6)?,
        longitude: r.get(7)?,
        is_electric: r.get::<_, Option<bool>>(8)?.unwrap_or(false),
    })
}
