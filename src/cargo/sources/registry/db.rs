use crate::CargoResult;
use once_cell::sync::OnceCell;
use rusqlite::{params, Connection};
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
        Ok(self.0.query_row(
            "SELECT contents FROM summaries WHERE name = ? LIMIT 1",
            [key],
            |row| row.get(0),
        )?)
    }

    pub fn insert<K>(&self, key: K, value: &[u8]) -> CargoResult<()>
    where
        K: AsRef<[u8]>,
    {
        let key = key.as_ref();
        let modified = self.0.execute(INSERT_SUMMERIES, params![key, value])?;
        log::debug!(
            "insert {} record for {}",
            modified,
            String::from_utf8_lossy(key)
        );
        Ok(())
    }
}
