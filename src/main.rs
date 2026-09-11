mod utils;

use blake3::Hasher;
use clap::{Parser, Subcommand};
use redb::{
    Database, DatabaseError, Error, ReadableDatabase, ReadableTable, ReadableTableMetadata,
    TableDefinition,
};
use redb_derive::Value;
use std::{
    env,
    fs::{self},
    io::{self, BufWriter, Read, Write},
    path::Path,
    thread::sleep,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const DB_NAME: &str = "history.redb";
const TABLE: TableDefinition<String, ClipboardMeta> = TableDefinition::new("bn");
const BLOB: TableDefinition<String, Vec<u8>> = TableDefinition::new("bn_blob");

const OPEN_ATTEMPTS: u32 = 100;
const OPEN_WAIT_START: Duration = Duration::from_millis(2);
const OPEN_WAIT_MAX: Duration = Duration::from_millis(50);

#[derive(Debug, Value, PartialEq, Eq, PartialOrd, Ord, Clone)]
struct ClipboardMeta {
    preview: String,
    mime: String,
    pinned: bool,
    timestamp: u128,
}

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[command(subcommand)]
    command: Commands,
}
#[derive(Subcommand, Debug)]
enum Commands {
    /// Store a clipboard entry (raw bytes from stdin)
    Store,

    /// List all the clipboard entries [hash]\t[mime]\t[pinned]\t[timestamp]\t[preview]
    List,

    /// Decode a clipboard entry by its hash
    Decode { hash: String },

    /// Delete a clipboard entry by its hash
    Delete { hash: String },

    /// Pin or unpin a clipboard entry by its hash
    Pin { hash: String },

    /// Clears all the history
    Wipe,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("Oops : {e}");
        std::process::exit(1)
    }
}

fn run() -> Result<(), Error> {
    let args = Args::parse();
    let config = utils::load_config();

    let stdin_bytes: Option<Vec<u8>> = match args.command {
        Commands::Store => {
            let limit = config.max_store_size.as_u64().saturating_add(1);
            let mut content: Vec<u8> = Vec::new();
            io::stdin().take(limit).read_to_end(&mut content)?;
            Some(content)
        }
        _ => None,
    };

    let cache_dir = env::home_dir()
        .unwrap()
        .join(".cache")
        .join(env!("CARGO_PKG_NAME"));
    fs::create_dir_all(&cache_dir)?;

    let db_path = cache_dir.join(DB_NAME);
    let db = open_db_retry(&db_path)?;

    let create_txn = db.begin_write()?;
    {
        create_txn.open_table(TABLE)?;
        create_txn.open_table(BLOB)?;
    }
    create_txn.commit()?;

    match args.command {
        Commands::Store => store(&db, &config, stdin_bytes.unwrap_or_default()),
        Commands::List => list(&db, &config),
        Commands::Decode { hash } => decode(&db, &hash),
        Commands::Delete { hash } => delete(&db, &hash),
        Commands::Pin { hash } => pin(&db, &hash),
        Commands::Wipe => wipe(&db),
    }
}

fn open_db_retry(path: &Path) -> Result<Database, Error> {
    let mut wait = OPEN_WAIT_START;
    let mut attempt: u32 = 0;
    loop {
        match Database::create(path) {
            Ok(db) => return Ok(db),
            Err(DatabaseError::DatabaseAlreadyOpen) if attempt < OPEN_ATTEMPTS => {
                attempt += 1;
                sleep(wait);
                wait = (wait * 2).min(OPEN_WAIT_MAX);
            }
            Err(e) => return Err(e.into()),
        }
    }
}

fn store(db: &Database, config: &utils::Config, content: Vec<u8>) -> Result<(), Error> {
    match std::env::var("CLIPBOARD_STATE")
        .unwrap_or_default()
        .as_str()
    {
        "clear" | "sensitive" => return Ok(()),
        _ => (),
    };

    let size = content.len() as u64;
    if size > config.max_store_size.as_u64() {
        return Ok(());
    }

    if size < config.min_store_size.as_u64() {
        return Ok(());
    }

    if !config.ignore_patterns.is_empty() {
        if let Ok(text) = std::str::from_utf8(&content) {
            if utils::should_ignore(text, &config.ignore_patterns) {
                return Ok(());
            }
        }
    }

    let content = utils::normalize_paths(&content).unwrap_or(content);

    let mut buffer_hash = [0u8; 8];
    Hasher::new()
        .update(&content)
        .finalize_xof()
        .fill(&mut buffer_hash);
    let content_hash = hex::encode(buffer_hash);

    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Oops : seems time went backwards")
        .as_nanos();

    let write_txn = db.begin_write()?;
    {
        let mut table = write_txn.open_table(TABLE)?;
        let duplicate = table.get(&content_hash)?.map(|entry| {
            let mut meta = entry.value();
            meta.timestamp = timestamp;
            meta
        });
        if let Some(meta) = duplicate {
            table.insert(&content_hash, &meta)?;
            drop(table);
            write_txn.commit()?;
            return Ok(());
        }

        let kind = utils::detect_mime(&content);
        let mime = match kind {
            Some(m) => m,
            None => return Ok(()),
        };
        let preview = utils::create_preview(&content, &mime);

        let mut blob = write_txn.open_table(BLOB)?;
        table.insert(
            &content_hash,
            &ClipboardMeta {
                preview,
                mime,
                pinned: false,
                timestamp,
            },
        )?;
        blob.insert(&content_hash, &content)?;

        loop {
            if table.len()? <= config.max_history_length {
                break;
            }
            match find_eviction_candidate(&table)? {
                Some(hash) => {
                    table.remove(&hash)?;
                    blob.remove(&hash)?;
                }
                None => break,
            }
        }
    }
    write_txn.commit()?;

    Ok(())
}

fn find_eviction_candidate(
    table: &redb::Table<String, ClipboardMeta>,
) -> Result<Option<String>, Error> {
    let mut oldest_unpinned: Option<(u128, String)> = None;
    let mut oldest_overall: Option<(u128, String)> = None;
    for row in table.iter()? {
        let (hash, meta) = row?;
        let meta = meta.value();
        let candidate = (meta.timestamp, hash.value());
        if !meta.pinned && oldest_unpinned.as_ref().is_none_or(|old| candidate < *old) {
            oldest_unpinned = Some(candidate.clone());
        }
        if oldest_overall.as_ref().is_none_or(|old| candidate < *old) {
            oldest_overall = Some(candidate);
        }
    }
    Ok(oldest_unpinned.or(oldest_overall).map(|(_, hash)| hash))
}

fn with_read<T>(
    db: &Database,
    f: impl FnOnce(
        &redb::ReadOnlyTable<String, ClipboardMeta>,
        &redb::ReadOnlyTable<String, Vec<u8>>,
    ) -> Result<T, Error>,
) -> Result<T, Error> {
    let txn = db.begin_read()?;
    let table = txn.open_table(TABLE)?;
    let blob = txn.open_table(BLOB)?;
    f(&table, &blob)
}

fn with_write<T>(
    db: &Database,
    f: impl FnOnce(
        &mut redb::Table<String, ClipboardMeta>,
        &mut redb::Table<String, Vec<u8>>,
    ) -> Result<T, Error>,
) -> Result<T, Error> {
    let txn = db.begin_write()?;
    let mut table = txn.open_table(TABLE)?;
    let mut blob = txn.open_table(BLOB)?;
    let out = f(&mut table, &mut blob)?;
    drop(table);
    drop(blob);
    txn.commit()?;
    Ok(out)
}

fn list(db: &Database, config: &utils::Config) -> Result<(), Error> {
    let mut rows: Vec<(String, ClipboardMeta)> = with_read(db, |table, _| {
        let mut rows = Vec::new();
        for row in table.iter()? {
            let (hash, meta) = row?;
            rows.push((hash.value(), meta.value()));
        }
        Ok(rows)
    })?;

    rows.sort_by(|a, b| {
        b.1.timestamp
            .cmp(&a.1.timestamp)
            .then_with(|| b.0.cmp(&a.0))
    });

    let width = config.preview_width.min(utils::PREVIEW_STORE_LEN);

    let stdout = io::stdout();
    let mut out = BufWriter::new(stdout.lock());

    for (hash, meta) in &rows {
        let is_plain_text = meta.mime.starts_with("text/");
        let preview = utils::truncate_preview(&meta.preview, width, is_plain_text);
        writeln!(
            out,
            "{}\t{}\t{}\t{}\t{}",
            hash, meta.mime, meta.pinned, meta.timestamp, preview
        )?;
    }
    out.flush()?;

    Ok(())
}

fn decode(db: &Database, hash: &str) -> Result<(), Error> {
    let key = hash.to_string();
    let bytes: Option<Vec<u8>> = with_read(db, |_, blob| Ok(blob.get(&key)?.map(|v| v.value())))?;
    if let Some(bytes) = bytes {
        io::stdout().write_all(&bytes)?
    }

    Ok(())
}

fn delete(db: &Database, hash: &str) -> Result<(), Error> {
    let key = hash.to_string();
    with_write(db, |table, blob| {
        table.remove(&key)?;
        blob.remove(&key)?;
        Ok(())
    })
}

fn pin(db: &Database, hash: &str) -> Result<(), Error> {
    let key = hash.to_string();
    with_write(db, |table, _| {
        let toggled = table.get(&key)?.map(|entry| {
            let mut meta = entry.value();
            meta.pinned = !meta.pinned;
            meta
        });
        if let Some(meta) = toggled {
            table.insert(key.clone(), meta)?;
        }
        Ok(())
    })
}

fn wipe(db: &Database) -> Result<(), Error> {
    let wipe_txn = db.begin_write()?;
    wipe_txn.delete_table(TABLE)?;
    wipe_txn.delete_table(BLOB)?;
    wipe_txn.commit()?;

    let create_txn = db.begin_write()?;
    create_txn.open_table(TABLE)?;
    create_txn.open_table(BLOB)?;
    create_txn.commit()?;

    Ok(())
}
