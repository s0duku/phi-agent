use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, params};

use super::{
    PhiHome, PhiHomeDoctorReport, PhiHomeEntry, PhiHomeError, PhiHomePath, PhiHomeResult,
    PhiHomeUrl,
};

const SQLITE_HEADER: &[u8] = b"SQLite format 3\0";

pub struct SqlitePhiHome {
    path: PathBuf,
}

impl SqlitePhiHome {
    pub fn from_path(path: PathBuf) -> Result<Self, Box<dyn std::error::Error>> {
        let home = Self { path };
        home.initialize()?;
        Ok(home)
    }

    pub fn from_entries(
        path: PathBuf,
        entries: &[PhiHomeEntry],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let home = Self::from_path(path)?;
        let mut conn = home.open_rw()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM phi_home_entries", [])?;
        for entry in entries {
            tx.execute(
                "INSERT OR REPLACE INTO phi_home_entries (path, content) VALUES (?1, ?2)",
                params![entry.path().as_str(), entry.content()],
            )?;
        }
        tx.commit()?;
        Ok(home)
    }

    fn initialize(&self) -> Result<(), Box<dyn std::error::Error>> {
        let conn = self.open_rw()?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS phi_home_entries (
                path TEXT PRIMARY KEY,
                content BLOB NOT NULL
            );",
        )?;
        Ok(())
    }

    fn open_rw(&self) -> Result<Connection, Box<dyn std::error::Error>> {
        Ok(Connection::open(&self.path)?)
    }

    fn open_ro(&self) -> Result<Connection, Box<dyn std::error::Error>> {
        Ok(Connection::open_with_flags(
            &self.path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?)
    }
}

impl PhiHome for SqlitePhiHome {
    fn doctor_report(&self) -> PhiHomeDoctorReport {
        let root = self.path.display().to_string();
        PhiHomeDoctorReport {
            kind: "sqlite".to_string(),
            root: root.clone(),
            source: "explicit".to_string(),
            config_path: format!("{root}#/config.yml"),
            tmp_path: format!("{root}#/tmp"),
        }
    }

    fn read_file(&self, source: &PhiHomeUrl) -> PhiHomeResult<Vec<u8>> {
        if source.scheme() != "phidb" {
            return Err(PhiHomeError::read(format!(
                "sqlite phi home only supports phidb urls, got {}",
                source.scheme()
            )));
        }

        let conn = self.open_ro().map_err(|error| {
            PhiHomeError::read(format!(
                "failed to open sqlite phi home {}: {error}",
                self.path.display()
            ))
        })?;
        conn.query_row(
            "SELECT content FROM phi_home_entries WHERE path = ?1",
            params![source.path()],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(|error| {
            PhiHomeError::read(format!(
                "failed to query sqlite phi home {}: {error}",
                self.path.display()
            ))
        })?
        .ok_or_else(|| {
            PhiHomeError::not_found(format!(
                "sqlite phi home entry not found: {}",
                source.path()
            ))
        })
    }

    fn entries(&self) -> Result<Vec<PhiHomeEntry>, Box<dyn std::error::Error>> {
        let conn = self.open_ro()?;
        let mut stmt =
            conn.prepare("SELECT path, content FROM phi_home_entries ORDER BY path ASC")?;
        let rows = stmt.query_map([], |row| {
            let path = PhiHomePath::new(row.get::<_, String>(0)?)
                .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
            let content = row.get::<_, Vec<u8>>(1)?;
            Ok(PhiHomeEntry::new(path, content))
        })?;

        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    fn url_for_path(&self, path: &PhiHomePath) -> PhiHomeUrl {
        PhiHomeUrl::new("phidb", path.as_str())
    }
}

pub fn looks_like_sqlite_home(path: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    if path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "sqlite" | "db" | "phihome"))
    {
        return Ok(true);
    }

    if !path.exists() || path.is_dir() {
        return Ok(false);
    }

    let bytes = std::fs::read(path)?;
    Ok(bytes.starts_with(SQLITE_HEADER))
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::SqlitePhiHome;
    use crate::home::{PhiHome, PhiHomeEntry, spec};

    #[test]
    fn from_entries_round_trips_canonical_home_entries() {
        let path = unique_temp_path("phi-home-sqlite");
        let entries = vec![PhiHomeEntry::new(
            spec::config_path(),
            b"model:\n  name: demo\n".to_vec(),
        )];

        let home = SqlitePhiHome::from_entries(path.clone(), &entries)
            .expect("sqlite phi home should be constructible from canonical entries");
        assert_eq!(home.entries().expect("entries should read back"), entries);

        std::fs::remove_file(path).expect("temp sqlite home should be removable");
    }

    fn unique_temp_path(prefix: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after unix epoch")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{nanos}.sqlite"))
    }
}
