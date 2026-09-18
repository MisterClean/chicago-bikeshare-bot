#![allow(clippy::unwrap_used, clippy::expect_used)]
use crate::{
    config::Config,
    database::Database,
    domain::{self, Station, Style},
    source,
};
use rusqlite::params;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    path::PathBuf,
    thread,
};
use tempfile::TempDir;

pub fn station(id: &str) -> Station {
    Station {
        id: id.into(),
        station_name: "State St & Lake St".into(),
        short_name: "TA1305000029".into(),
        total_docks: 23,
        docks_in_service: 23,
        status: "In Service".into(),
        latitude: 41.885,
        longitude: -87.628,
        is_electric: false,
    }
}

#[test]
fn long_station_alerts_keep_notices_within_post_limit() {
    for kind in [
        domain::EventType::Discovered,
        domain::EventType::Electrified,
    ] {
        let mut station = station("inactive");
        station.station_name = "é🚲".repeat(60);
        station.status = "Not In Service".into();
        station.is_electric = true;
        let payload = domain::Payload {
            kind,
            station,
            observed_at: "2026-09-18T00:00:00.000Z".into(),
        };
        let text = domain::post_text(&payload);
        assert!(text.chars().count() <= 300);
        assert!(text.contains("Unofficial • Chicago open data"));
        assert!(text.contains("Charging inferred from source name."));
        assert!(text.ends_with("Not live availability; check the official app."));
        assert!(!text.contains("now has charging docks"));
    }
}
fn database() -> (TempDir, Database) {
    let dir = TempDir::new().unwrap();
    let db = Database::open(&dir.path().join("bot.sqlite3")).unwrap();
    (dir, db)
}
fn queue(db: &mut Database) {
    db.reconcile(&[station("old")], "2026-01-01T00:00:00.000Z")
        .unwrap();
    db.reconcile(
        &[station("new"), station("other")],
        "2026-01-02T00:00:00.000Z",
    )
    .unwrap();
}
#[test]
fn baseline_is_silent() {
    let (_dir, mut db) = database();
    let summary = db
        .reconcile(&[station("1")], "2026-01-01T00:00:00.000Z")
        .unwrap();
    assert!(summary.baseline_created);
    assert!(db.ready(10).unwrap().is_empty());
}
#[test]
fn missing_historical_stations_are_retained() {
    let (_dir, mut db) = database();
    queue(&mut db);
    assert_eq!(db.count().unwrap(), 3);
    assert_eq!(db.station("old").unwrap(), Some(station("old")));
}
#[test]
fn reconciliation_is_idempotent_and_preserves_first_seen() {
    let (_dir, mut db) = database();
    queue(&mut db);
    db.reconcile(&[station("new")], "2026-02-01T00:00:00.000Z")
        .unwrap();
    assert_eq!(db.ready(10).unwrap().len(), 2);
    let first: String = db
        .connection
        .query_row(
            "SELECT first_seen_at FROM stations WHERE id='new'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(first, "2026-01-02T00:00:00.000Z");
}
#[test]
fn electrification_is_announced_only_once() {
    let (_dir, mut db) = database();
    let mut s = station("1");
    db.reconcile(&[s.clone()], &domain::now()).unwrap();
    s.is_electric = true;
    let summary = db.reconcile(&[s.clone()], &domain::now()).unwrap();
    assert_eq!(summary.electrified, 1);
    s.is_electric = false;
    db.reconcile(&[s.clone()], &domain::now()).unwrap();
    s.is_electric = true;
    db.reconcile(&[s], &domain::now()).unwrap();
    assert_eq!(db.ready(10).unwrap().len(), 1);
}
#[test]
fn styles_alternate_and_stay_persisted() {
    let (_dir, mut db) = database();
    queue(&mut db);
    let deliveries = db.ready(10).unwrap();
    assert_eq!(deliveries[0].style, Style::Civic);
    assert_eq!(deliveries[1].style, Style::Nightline);
    assert!(db.claim(&deliveries[0].id).unwrap());
    db.retry(&deliveries[0].id, "failure").unwrap();
    let style: String = db
        .connection
        .query_row(
            "SELECT announcement_style FROM deliveries WHERE id=?",
            [&deliveries[0].id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(style, "civic");
}
#[test]
fn malformed_snapshot_does_not_partially_update_state() {
    let (_dir, mut db) = database();
    queue(&mut db);
    let mut bad = station("invalid");
    bad.latitude = 0.;
    assert!(
        db.reconcile(&[station("new-valid"), bad], &domain::now())
            .is_err()
    );
    assert!(db.station("new-valid").unwrap().is_none());
}
#[test]
fn duplicate_snapshot_rejected() {
    let (_dir, mut db) = database();
    assert!(
        db.reconcile(&[station("1"), station("1")], &domain::now())
            .is_err()
    );
    assert_eq!(db.count().unwrap(), 0);
}
#[test]
fn existing_database_is_not_migrated_or_rewritten_on_open() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("v2.sqlite3");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(include_str!("../tests/fixtures/schema-v2.sql"))
        .unwrap();
    drop(conn);
    let before = fs::read(&path).unwrap();
    drop(Database::open(&path).unwrap());
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn readonly_status_never_initializes_missing_database() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("missing.sqlite3");
    assert!(Database::open_readonly(&path).is_err());
    assert!(!path.exists());
}
#[test]
fn unknown_schema_rejected_without_changes() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("unknown.sqlite3");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE stations(id TEXT)")
        .unwrap();
    drop(connection);
    let before = fs::read(&path).unwrap();
    assert!(Database::open(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), before);
}
#[test]
fn process_lock_prevents_overlapping_runs() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bot.sqlite3");
    let first = Database::open(&path).unwrap();
    assert!(Database::open(&path).is_err());
    drop(first);
    assert!(Database::open(&path).is_ok());
}
#[test]
fn retry_is_delayed_and_abandoned_claim_is_recovered() {
    let (_dir, mut db) = database();
    queue(&mut db);
    let d = db.ready(1).unwrap().remove(0);
    assert!(db.claim(&d.id).unwrap());
    assert!(!db.claim(&d.id).unwrap());
    db.retry(&d.id, "offline").unwrap();
    assert!(db.ready(10).unwrap().iter().all(|r| r.id != d.id));
    db.connection.execute("UPDATE deliveries SET status='processing',locked_at='2020-01-01T00:00:00.000Z' WHERE id=?",[&d.id]).unwrap();
    assert_eq!(db.recover().unwrap(), 1);
    assert_eq!(db.ready(1).unwrap()[0].record_key, d.record_key);
}
#[test]
fn delivery_is_not_retried_after_success() {
    let (_dir, mut db) = database();
    queue(&mut db);
    let d = db.ready(1).unwrap().remove(0);
    db.claim(&d.id).unwrap();
    db.delivered(&d.id, "at://did:plc:test/app.bsky.feed.post/key", "cid")
        .unwrap();
    assert!(db.ready(10).unwrap().iter().all(|r| r.id != d.id));
    assert!(!db.claim(&d.id).unwrap());
}
#[test]
fn persisted_typescript_payload_is_compatible() {
    let s = r#"{"type":"station.discovered","station":{"id":"2265023285046353730","stationName":"Racine Ave & 82nd St","shortName":"CHI02416","totalDocks":8,"docksInService":8,"status":"In Service","latitude":41.74488,"longitude":-87.65371,"isElectric":false},"observedAt":"2026-09-18T12:00:09.796Z"}"#;
    let payload: domain::Payload = serde_json::from_str(s).unwrap();
    assert_eq!(payload.station.id, "2265023285046353730");
    assert_eq!(
        serde_json::to_value(payload).unwrap(),
        serde_json::from_str::<serde_json::Value>(s).unwrap()
    );
}
#[test]
fn reply_key_matches_legacy_clock_modulo_32() {
    for clock in [0, 1, 31, 32, 99, 1023] {
        let root = domain::encode_tid((1700000000000000u64 << 10) | clock);
        let reply = domain::decode_tid(&domain::reply_key(&root).unwrap()).unwrap();
        assert_eq!(reply & 1023, (clock + 1) % 32);
        assert_eq!(reply >> 10, 1700000000000000);
    }
}
#[test]
fn source_rejects_empty_partial_duplicate_and_invalid_records() {
    assert!(source::parse(b"[]", 1).is_err());
    let record = serde_json::json!({"id":"large-id-2265023285046353730","station_name":"Station* ","short_name":"a","total_docks":"8","docks_in_service":"8","status":"In Service","latitude":"41.74488","longitude":"-87.65371"});
    let one = serde_json::to_vec(&[&record]).unwrap();
    assert!(source::parse(&one, 2).is_err());
    assert!(source::parse(&serde_json::to_vec(&[&record, &record]).unwrap(), 1).is_err());
    assert!(source::parse(&one, 1).unwrap()[0].is_electric);
}
#[test]
fn config_rejects_missing_credentials_and_huge_canvas() {
    let mut env = HashMap::new();
    env.insert("PUBLISH_ENABLED".into(), "true".into());
    assert!(Config::from_map(&env).is_err());
    env.clear();
    env.insert("MAP_PIXEL_RATIO".into(), "3".into());
    assert!(Config::from_map(&env).is_err());
}
#[test]
fn redaction_does_not_expose_credentials() {
    let config = Config::from_map(&HashMap::from([(
        "PROTOMAPS_KEY".into(),
        "secret map key".into(),
    )]))
    .unwrap();
    assert!(
        !config
            .redact("secret map key and secret+map+key")
            .contains("secret")
    );
}
#[test]
fn image_encoding_has_correct_dimensions_and_byte_limit() {
    let bytes = crate::media::jpeg(&vec![255; 600 * 400 * 3], 600, 400).unwrap();
    assert!(bytes.len() <= crate::media::MAX_IMAGE_BYTES);
    let image = image::load_from_memory(&bytes).unwrap();
    assert_eq!((image.width(), image.height()), (600, 400));
}
#[test]
fn geometry_rejects_truncated_and_invalid_commands() {
    let feature = crate::vector_tile::Feature {
        tags: vec![],
        kind: Some(2),
        geometry: vec![9, 10],
    };
    assert!(crate::vector_tile::geometry(&feature).is_err());
    let feature = crate::vector_tile::Feature {
        geometry: vec![15],
        ..feature
    };
    assert!(crate::vector_tile::geometry(&feature).is_err());
}
#[test]
fn run_history_keeps_failure_counts() {
    let (_dir, db) = database();
    let run = db.start_run().unwrap();
    db.finish_run(&run, 1210, 3, 2, Some("one failure"))
        .unwrap();
    let result: (String, i64) = db
        .connection
        .query_row(
            "SELECT status,deliveries_completed FROM runs WHERE id=?",
            [run],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(result, ("failed".into(), 2));
}
#[test]
fn event_transaction_rolls_back_when_delivery_insert_fails() {
    let (_dir, mut db) = database();
    db.reconcile(&[station("old")], &domain::now()).unwrap();
    db.connection.execute_batch("CREATE TRIGGER reject_delivery BEFORE INSERT ON deliveries BEGIN SELECT RAISE(ABORT,'test failure'); END;").unwrap();
    assert!(db.reconcile(&[station("new")], &domain::now()).is_err());
    assert!(db.station("new").unwrap().is_none());
    let count: i64 = db
        .connection
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

pub fn mock_server(responses: Vec<(u16, String)>) -> (String, thread::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        for (status, body) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            requests.push(String::from_utf8_lossy(&request).into_owned());
            write!(stream,"HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
        requests
    });
    (url, handle)
}
#[test]
fn no_publish_run_uses_existing_schema_and_preserves_delivered_history() {
    let (_dir, mut db) = database();
    queue(&mut db);
    let d = db.ready(1).unwrap().remove(0);
    db.claim(&d.id).unwrap();
    db.delivered(&d.id, "uri", "cid").unwrap();
    let path: PathBuf = db.connection.path().unwrap().into();
    drop(db);
    let body=serde_json::json!([{"id":"new","station_name":"New name","short_name":"a","total_docks":"9","docks_in_service":"9","status":"In Service","latitude":"41.7","longitude":"-87.6"}]).to_string();
    let (url, handle) = mock_server(vec![(200, body)]);
    let config = Config::from_map(&HashMap::from([
        ("DB_PATH".into(), path.to_string_lossy().into()),
        ("SODA_URL".into(), url),
        ("SODA_MIN_STATIONS".into(), "1".into()),
    ]))
    .unwrap();
    crate::run(&config).unwrap();
    handle.join().unwrap();
    let db = Database::open_readonly(&path).unwrap();
    db.check().unwrap();
    let persisted: (String, String) = db
        .connection
        .query_row(
            "SELECT record_key,post_uri FROM deliveries WHERE id=?",
            params![d.id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(persisted, (d.record_key, "uri".into()));
    assert_eq!(db.count().unwrap(), 3);
}
