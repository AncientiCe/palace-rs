//! Backend contracts for storage implementations.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::store::{Drawer, DrawerFilter, SearchResult};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PalaceRef {
    pub backend: String,
    pub location: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetResult {
    pub drawer: Option<Drawer>,
}

pub trait Backend {
    fn palace_ref(&self) -> PalaceRef;
    fn count_drawers(&self) -> Result<i64>;
    fn get_drawer(&self, id: &str) -> Result<GetResult>;
    fn query(&self, query_vec: &[f32], filter: &DrawerFilter, limit: usize) -> Result<QueryResult>;
}

pub trait SourceAdapter {
    fn name(&self) -> &str;
    fn ingest(&self) -> Result<usize>;
}
