//! SQLite backend implementation.

use anyhow::Result;
use rusqlite::Connection;
use std::path::{Path, PathBuf};

use crate::backend::{Backend, GetResult, PalaceRef, QueryResult};
use crate::store::{self, DrawerFilter};

pub struct SqliteBackend {
    conn: Connection,
    location: PathBuf,
}

impl SqliteBackend {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            conn: crate::db::open(path)?,
            location: path.to_path_buf(),
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        Ok(Self {
            conn: crate::db::open_in_memory()?,
            location: PathBuf::from(":memory:"),
        })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }
}

impl Backend for SqliteBackend {
    fn palace_ref(&self) -> PalaceRef {
        PalaceRef {
            backend: "sqlite".to_string(),
            location: self.location.to_string_lossy().into_owned(),
        }
    }

    fn count_drawers(&self) -> Result<i64> {
        store::count_drawers(&self.conn)
    }

    fn get_drawer(&self, id: &str) -> Result<GetResult> {
        Ok(GetResult {
            drawer: store::get_drawer(&self.conn, id)?,
        })
    }

    fn query(&self, query_vec: &[f32], filter: &DrawerFilter, limit: usize) -> Result<QueryResult> {
        Ok(QueryResult {
            results: store::vector_search(&self.conn, query_vec, filter, limit)?,
        })
    }
}
