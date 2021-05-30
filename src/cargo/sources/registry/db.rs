use crate::CargoResult;
use once_cell::sync::OnceCell;
use rusqlite::blob::{Blob, ZeroBlob};
use rusqlite::{params, Connection, DatabaseName};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Mutex;

pub(crate) struct Db(Connection);

const TABLE_SUMMARIES: &'static str = "\
CREATE TABLE IF NOT EXISTS summaries (
    name TEXT PRIMARY KEY NOT NULL,
    contents BLOB NOT NULL
)";

const INSERT_SUMMERIES: &'static str = "\
INSERT OR REPLACE INTO summaries (name, contents) VALUES (?, ?)";

impl Db {
    pub fn open<P>(path: P) -> CargoResult<&'static Mutex<Self>>
    where
        P: AsRef<Path>,
    {
        static DB: OnceCell<Mutex<Db>> = OnceCell::new();
        DB.get_or_try_init(|| {
            let conn = Connection::open(path.as_ref())?;
            conn.pragma_update(None, "locking_mode", &"EXCLUSIVE")?;
            conn.pragma_update(None, "cache_size", &2048)?;
            conn.execute(TABLE_SUMMARIES, [])?;
            Ok(Mutex::new(Self(conn)))
        })
    }

    pub fn get<K>(&self, key: K) -> CargoResult<Vec<u8>>
    where
        K: AsRef<[u8]>,
    {
        let key = key.as_ref();
        let row_id = self.0.query_row(
            "SELECT rowid FROM summaries WHERE name = ? LIMIT 1",
            [key],
            |row| row.get(0),
        )?;
        let mut blob = self.blob_open(row_id, false)?;
        let len = blob.len();
        let mut buf = Vec::with_capacity(len);
        let bytes_read = blob.read_to_end(&mut buf)?;
        assert_eq!(bytes_read, len);
        Ok(buf)
    }

    pub fn insert<K>(&self, key: K, value: &[u8]) -> CargoResult<()>
    where
        K: AsRef<[u8]>,
    {
        let key = key.as_ref();
        let zblob = ZeroBlob(value.len() as i32);
        let modified = self.0.execute(INSERT_SUMMERIES, params![key, zblob])?;
        let row_id = self.0.last_insert_rowid();
        let mut blob = self.blob_open(row_id, false)?;
        let bytes_written = blob.write(value)?;
        assert_eq!(bytes_written, value.len());
        log::debug!(
            "insert {} record for {}",
            modified,
            String::from_utf8_lossy(key)
        );
        Ok(())
    }

    fn blob_open(&self, row_id: i64, read_only: bool) -> CargoResult<Blob<'_>> {
        Ok(self.0.blob_open(
            DatabaseName::Main,
            "summaries",
            "contents",
            row_id,
            read_only,
        )?)
    }
}
