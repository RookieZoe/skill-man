//! SQLite adapter for the Scan Managed Facts reader (spec §8.2): opens the
//! active Bound Home's Catalog read-only at call time (Reconnect/Restore
//! never leave it pointing at a stale path), exactly like the Source
//! Capability scan. It reads `directory_name` + `final_entity_path` of every
//! skill — the durable pointer facts — and never writes.

use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::{Connection, OpenFlags, params};

use crate::core::write_gate::WriteGate;
use crate::seams::filesystem::FileSystem;
use crate::seams::scan_managed_facts::{ManagedSkillPathFact, ScanManagedFactsReader};

pub struct SqliteScanManagedFactsReader {
    home_context: Arc<WriteGate>,
    catalog_file_name: String,
    filesystem: Arc<dyn FileSystem>,
}

impl SqliteScanManagedFactsReader {
    pub fn new(
        home_context: Arc<WriteGate>,
        catalog_file_name: impl Into<String>,
        filesystem: Arc<dyn FileSystem>,
    ) -> Self {
        Self {
            home_context,
            catalog_file_name: catalog_file_name.into(),
            filesystem,
        }
    }
}

impl ScanManagedFactsReader for SqliteScanManagedFactsReader {
    fn read(&self) -> Result<Vec<ManagedSkillPathFact>, String> {
        let home = self
            .home_context
            .bound_home()
            .map_err(|error| error.to_string())?;
        let catalog_path = home.path.join(&self.catalog_file_name);
        let connection = Connection::open_with_flags(
            &catalog_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| format!("open {} read-only: {error}", catalog_path.display()))?;
        let mut statement = connection
            .prepare(
                "SELECT directory_name, final_entity_path FROM skills ORDER BY directory_name",
            )
            .map_err(|error| error.to_string())?;
        let rows = statement
            .query_map(params![], |row| {
                let directory_name: String = row.get(0)?;
                let final_entity_path: String = row.get(1)?;
                Ok((directory_name, final_entity_path))
            })
            .map_err(|error| error.to_string())?;
        let mut facts = Vec::new();
        for row in rows {
            let (directory_name, final_entity_path) = row.map_err(|error| error.to_string())?;
            let path = PathBuf::from(&final_entity_path);
            let canonical = self
                .filesystem
                .canonical_directory(&path)
                .unwrap_or(path);
            facts.push(ManagedSkillPathFact {
                directory_name,
                final_entity_path: canonical,
            });
        }
        Ok(facts)
    }
}
