use crate::{
    backend::PreparedChange, composition::Compiled, model::validate_id, reconcile::UnitState,
    require, Error, Result,
};
use nickel_util::files;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use std::{
    fs::{File, OpenOptions},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub struct Store {
    connection: Connection,
    directory: File,
    _lease: File,
    root: PathBuf,
    pub owner: String,
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        let root = files::absolute(root)?;
        if !root.exists() {
            let parent = files::open_directory(
                root.parent()
                    .ok_or_else(|| Error::Invalid("Store parent required".into()))?,
            )?;
            let name = root
                .file_name()
                .ok_or_else(|| Error::Invalid("Store name required".into()))?;
            std::fs::DirBuilder::new().mode(0o700).create(
                PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name),
            )?;
            parent.sync_all()?;
        }
        let directory = files::open_directory(&root)?;
        require(
            directory.metadata()?.mode() & 0o077 == 0,
            "Store directory must be private (0700)",
        )?;
        let pinned = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(pinned.join("controller.lock"))?;
        require(
            lease.metadata()?.is_file() && lease.metadata()?.nlink() == 1,
            "Invalid controller lock file",
        )?;
        // SAFETY: lease owns a live file descriptor. flock is released when it closes.
        if unsafe { libc::flock(lease.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(Error::Conflict("Another controller owns this store".into()));
        }
        let db = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
            .open(pinned.join("store.sqlite"))?;
        require(
            db.metadata()?.is_file() && db.metadata()?.nlink() == 1,
            "Invalid store database",
        )?;
        require(
            db.metadata()?.mode() & 0o077 == 0,
            "Store database must be private",
        )?;
        let connection = Connection::open_with_flags(
            root.join("store.sqlite"),
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        let current = files::open_directory(&root)?;
        require(
            current.metadata()?.ino() == directory.metadata()?.ino()
                && current.metadata()?.dev() == directory.metadata()?.dev(),
            "Store directory changed during database open",
        )?;
        connection.busy_timeout(std::time::Duration::from_secs(2))?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        require(
            version == 0 || version == 1,
            "Unsupported store schema version",
        )?;
        if version == 0 {
            let tables: i64 = connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )?;
            require(tables == 0, "Refusing to initialize an unrelated database")?;
        }
        connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;
   CREATE TABLE IF NOT EXISTS metadata (key TEXT PRIMARY KEY,value TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS instances (id TEXT PRIMARY KEY,revision TEXT NOT NULL,compiled TEXT NOT NULL,deleted INTEGER NOT NULL DEFAULT 0);
   CREATE TABLE IF NOT EXISTS units (instance TEXT NOT NULL,unit TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(instance,unit));
   CREATE TABLE IF NOT EXISTS operations (id TEXT PRIMARY KEY,instance TEXT NOT NULL,unit TEXT NOT NULL,revision TEXT NOT NULL,phase TEXT NOT NULL,change TEXT NOT NULL);
   CREATE TABLE IF NOT EXISTS bindings (binding TEXT PRIMARY KEY,instance TEXT NOT NULL,unit TEXT NOT NULL);
   INSERT OR IGNORE INTO metadata VALUES('owner',lower(hex(randomblob(16)))); PRAGMA user_version=1;")?;
        let owner =
            connection.query_row("SELECT value FROM metadata WHERE key='owner'", [], |r| {
                r.get(0)
            })?;
        directory.sync_all()?;
        Ok(Self {
            connection,
            directory,
            _lease: lease,
            root,
            owner,
        })
    }
    pub fn path(&self) -> PathBuf {
        self.root.clone()
    }
    pub fn work(&self, instance: &str, unit: &str) -> Result<PathBuf> {
        validate_id(instance)?;
        validate_id(unit)?;
        let path = self.path().join(format!(
            "work-{}",
            files::hash(format!("{instance}/{unit}").as_bytes())
        ));
        if !path.exists() {
            std::fs::DirBuilder::new().mode(0o700).create(&path)?;
            self.directory.sync_all()?;
        }
        let name = path
            .file_name()
            .ok_or_else(|| Error::Invalid("Work name missing".into()))?;
        let child = files::open_child(&self.directory, name, true)?;
        require(
            child.metadata()?.mode() & 0o077 == 0,
            "Workspace must be private",
        )?;
        Ok(path)
    }
    pub fn submit(&mut self, compiled: &Compiled) -> Result<()> {
        compiled.verify()?;
        let data = serde_json::to_string(compiled)?;
        require(
            data.len() <= 16 * 1024 * 1024,
            "Compiled instance exceeds 16 MiB",
        )?;
        self.connection.execute("INSERT INTO instances(id,revision,compiled) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,compiled=excluded.compiled",params![compiled.instance.id,compiled.revision,data])?;
        Ok(())
    }
    pub fn load(&self, id: &str) -> Result<Compiled> {
        let data: String =
            self.connection
                .query_row("SELECT compiled FROM instances WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        let result: Compiled = serde_json::from_str(&data)?;
        result.verify()?;
        Ok(result)
    }
    pub fn ensure_revision(&self, id: &str, revision: &str) -> Result<()> {
        let actual: String =
            self.connection
                .query_row("SELECT revision FROM instances WHERE id=?1", [id], |r| {
                    r.get(0)
                })?;
        if actual != revision {
            return Err(Error::Conflict("Desired revision was superseded".into()));
        }
        Ok(())
    }
    pub fn token(&self) -> Result<String> {
        Ok(self
            .connection
            .query_row("SELECT lower(hex(randomblob(16)))", [], |r| r.get(0))?)
    }
    pub fn claim(&self, binding: &str, instance: &str, unit: &str) -> Result<()> {
        self.connection.execute(
            "INSERT OR IGNORE INTO bindings VALUES(?1,?2,?3)",
            params![binding, instance, unit],
        )?;
        let actual: (String, String) = self.connection.query_row(
            "SELECT instance,unit FROM bindings WHERE binding=?1",
            [binding],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if actual != (instance.into(), unit.into()) {
            return Err(Error::Conflict(
                "State binding is owned by another unit".into(),
            ));
        }
        Ok(())
    }
    pub fn unit(&self, instance: &str, unit: &str) -> Result<Option<UnitState>> {
        let data: Option<String> = self
            .connection
            .query_row(
                "SELECT state FROM units WHERE instance=?1 AND unit=?2",
                params![instance, unit],
                |r| r.get(0),
            )
            .optional()?;
        data.map(|data| serde_json::from_str(&data).map_err(Error::from))
            .transpose()
    }
    pub fn units(&self, instance: &str) -> Result<BTreeMap<String, UnitState>> {
        let mut statement = self
            .connection
            .prepare("SELECT unit,state FROM units WHERE instance=?1 ORDER BY unit")?;
        let rows = statement.query_map([instance], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut result = BTreeMap::new();
        for row in rows {
            let (id, data) = row?;
            result.insert(id, serde_json::from_str(&data)?);
        }
        Ok(result)
    }
    pub fn save_unit(&mut self, instance: &str, unit: &str, state: &UnitState) -> Result<()> {
        self.ensure_revision(instance, &state.revision)?;
        self.connection.execute("INSERT INTO units VALUES(?1,?2,?3) ON CONFLICT(instance,unit) DO UPDATE SET state=excluded.state",params![instance,unit,serde_json::to_string(state)?])?;
        Ok(())
    }
    pub fn record(
        &self,
        instance: &str,
        unit: &str,
        change: &PreparedChange,
        phase: &str,
    ) -> Result<()> {
        self.ensure_revision(instance, &change.revision)?;
        self.connection.execute("INSERT INTO operations VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET phase=excluded.phase",params![change.operation,instance,unit,change.revision,phase,serde_json::to_string(change)?])?;
        Ok(())
    }
    pub fn finish(&self, operation: &str, phase: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE operations SET phase=?1 WHERE id=?2",
            params![phase, operation],
        )?;
        Ok(())
    }
    pub fn running(&self, instance: &str, unit: &str) -> Result<Option<PreparedChange>> {
        let data:Option<String>=self.connection.query_row("SELECT change FROM operations WHERE instance=?1 AND unit=?2 AND phase='running' ORDER BY rowid DESC LIMIT 1",params![instance,unit],|r|r.get(0)).optional()?;
        data.map(|s| serde_json::from_str(&s).map_err(Error::from))
            .transpose()
    }
    pub fn mark_removed(&self, id: &str, delete: bool) -> Result<()> {
        let compiled = self.load(id)?;
        require(
            !delete
                || (compiled.instance.policy.deletion == crate::model::Deletion::Delete
                    && compiled.instance.policy.allow_delete),
            "Instance policy does not authorize deletion",
        )?;
        self.connection.execute(
            "UPDATE instances SET deleted=?1 WHERE id=?2",
            params![if delete { 2 } else { 1 }, id],
        )?;
        Ok(())
    }
    pub fn removal(&self, id: &str) -> Result<u8> {
        Ok(self
            .connection
            .query_row("SELECT deleted FROM instances WHERE id=?1", [id], |r| {
                r.get(0)
            })?)
    }
}
use std::collections::BTreeMap;

/// A read-only projection; works while the controller holds its exclusive writer lease.
pub fn status(root: &Path, id: &str) -> Result<serde_json::Value> {
    validate_id(id)?;
    let root = files::absolute(root)?;
    let directory = files::open_directory(&root)?;
    require(
        directory.metadata()?.mode() & 0o077 == 0,
        "Store must be private",
    )?;
    let db = files::open_child(&directory, std::ffi::OsStr::new("store.sqlite"), false)?;
    require(
        db.metadata()?.is_file()
            && db.metadata()?.nlink() == 1
            && db.metadata()?.mode() & 0o077 == 0,
        "Invalid store database",
    )?;
    let connection = Connection::open_with_flags(
        root.join("store.sqlite"),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(std::time::Duration::from_secs(2))?;
    let (revision, removal): (String, u8) = connection.query_row(
        "SELECT revision,deleted FROM instances WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut statement =
        connection.prepare("SELECT unit,state FROM units WHERE instance=?1 ORDER BY unit")?;
    let rows = statement.query_map([id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut units = BTreeMap::new();
    for row in rows {
        let (name, data) = row?;
        let state: UnitState = serde_json::from_str(&data)?;
        units.insert(name, state.public());
    }
    Ok(serde_json::json!({"instance":id,"revision":revision,"removal":removal,"units":units}))
}
