mod config;
mod database;
mod domain;
mod http;
mod media;
mod publisher;
mod source;
#[cfg(test)]
mod tests;
mod vector_tile;

use anyhow::{Context, Result, ensure};
use config::Config;
use database::Database;
use domain::{Style, now};

fn main() {
    if let Err(error) = execute() {
        eprintln!("{error:#}");
        std::process::exit(1);
    }
}
fn execute() -> Result<()> {
    match dotenvy::dotenv() {
        Ok(_) => {}
        Err(error) if error.not_found() => {}
        Err(_) => anyhow::bail!("Invalid .env file"),
    }
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    if arguments
        .first()
        .is_some_and(|arg| arg == "--help" || arg == "help")
    {
        println!(
            "chicago-bikeshare-bot [run | status | check | init | render STATION_ID [OUTPUT] [civic|nightline] | import-legacy PATH]\nrun: reconcile complete feed and process queue (publishing off by default)\nstatus/check/render: open an existing database read-only\ninit: explicitly initialize an empty v2 database"
        );
        return Ok(());
    }
    let config = Config::load()?;
    command(&config, &arguments)
        .map_err(|error| anyhow::anyhow!("{}", config.redact(&format!("{error:#}"))))
}
fn command(config: &Config, args: &[String]) -> Result<()> {
    let command = args.first().map_or("run", String::as_str);
    match command {
        "run" => run(config)?,
        "init" => {
            let db = Database::open(&config.db_path)?;
            println!("{}", db.status()?);
        }
        "status" | "check" => {
            let db = Database::open_readonly(&config.db_path)?;
            if command == "check" {
                db.check()?;
            }
            println!("{}", db.status()?);
        }
        "render" => {
            let id = args.get(1).context(
                "Usage: chicago-bikeshare-bot render STATION_ID [OUTPUT] [civic|nightline]",
            )?;
            let style = match args.get(3).map(String::as_str) {
                None | Some("civic") => Style::Civic,
                Some("nightline") => Style::Nightline,
                _ => anyhow::bail!("Unknown card style"),
            };
            let db = Database::open_readonly(&config.db_path)?;
            let station = db.station(id)?.context("Station not found")?;
            drop(db);
            let image = media::render(config, &station, style, false)?;
            let output = args
                .get(2)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    config
                        .output
                        .join(format!("{}.jpg", station.id.replace(['/', '\\'], "_")))
                });
            if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&output, image.bytes)?;
            println!("Rendered {}", output.display());
        }
        "import-legacy" => {
            let path = args
                .get(1)
                .context("Usage: chicago-bikeshare-bot import-legacy PATH")?;
            let source = rusqlite::Connection::open_with_flags(
                path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let stations=source.prepare("SELECT id,station_name,short_name,total_docks,docks_in_service,status,latitude,longitude,is_electric FROM stations ORDER BY id")?.query_map([],database::station_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            let mut db = Database::open(&config.db_path)?;
            ensure!(db.count()? == 0, "Legacy import requires an empty target");
            println!(
                "{}",
                serde_json::to_string(&db.reconcile(&stations, &now())?)?
            );
        }
        _ => anyhow::bail!("Unknown command; use --help"),
    }
    Ok(())
}
fn run(config: &Config) -> Result<()> {
    let mut db = Database::open(&config.db_path)?;
    let run_id = db.start_run()?;
    let (mut fetched, mut events, mut completed) = (0, 0, 0);
    let result = (|| -> Result<()> {
        let recovered = db.recover()?;
        if recovered > 0 {
            eprintln!("Recovered {recovered} abandoned deliveries");
        }
        let stations = source::fetch(config)?;
        fetched = stations.len();
        let summary = db.reconcile(&stations, &now())?;
        events = summary.events_created;
        println!("{}", serde_json::to_string(&summary)?);
        drop(stations);
        if config.publish {
            let mut publisher = publisher::Publisher::new(config);
            let mut failures = 0;
            for delivery in db.ready(config.post_limit)? {
                if !db.claim(&delivery.id)? {
                    continue;
                }
                match publisher.publish(&delivery) {
                    Ok(post) => {
                        db.delivered(&delivery.id, &post.uri, &post.cid)?;
                        completed += 1;
                        println!("Published {}", post.uri);
                    }
                    Err(error) => {
                        let message = config.redact(&format!("{error:#}"));
                        db.retry(&delivery.id, &message)?;
                        failures += 1;
                        eprintln!("Delivery {} requeued: {message}", delivery.id);
                    }
                }
            }
            ensure!(
                failures == 0,
                "{failures} deliveries failed and were requeued"
            );
        }
        Ok(())
    })();
    let error = result
        .as_ref()
        .err()
        .map(|error| config.redact(&format!("{error:#}")));
    db.finish_run(&run_id, fetched, events, completed, error.as_deref())?;
    println!(
        "{}",
        serde_json::json!({"stationsFetched":fetched,"eventsCreated":events,"deliveriesCompleted":completed,"status":if result.is_ok(){"succeeded"}else{"failed"}})
    );
    result
}
