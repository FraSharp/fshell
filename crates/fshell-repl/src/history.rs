// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Francesco Duca <f.duca00@gmail.com>

use chrono::{TimeZone, Utc};
use fshell_core::{FxIndexMap, Val};
use r2d2::ManageConnection;
use reedline::{
    CommandLineSearch, HistoryItem as ReedlineHistoryItem, HistorySessionId, SearchQuery,
};
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::time::Duration;
use ustr::ustr;

#[cfg(test)]
pub static TEST_DB_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[derive(Debug, Clone)]
pub struct HistoryEntry {
    pub id: i64,
    pub command: String,
    pub cwd: String,
    pub timestamp_ms: i64,
    pub duration_ms: i64,
    pub exit_code: Option<i64>,
    pub hostname: String,
    pub username: String,
    pub session_id: String,
}

impl HistoryEntry {
    pub fn to_val(&self) -> Val {
        let mut m = FxIndexMap::with_hasher(fshell_hash::FxBuildHasher::default());
        m.insert(ustr("id"), Val::Int(self.id));
        m.insert(ustr("command"), Val::String(self.command.clone()));
        m.insert(ustr("cwd"), Val::String(self.cwd.clone()));

        let datetime = Utc
            .timestamp_millis_opt(self.timestamp_ms)
            .single()
            .unwrap_or_else(Utc::now);
        m.insert(ustr("timestamp"), Val::DateTime(datetime));

        m.insert(ustr("duration_ms"), Val::Int(self.duration_ms));
        m.insert(
            ustr("exit_code"),
            match self.exit_code {
                Some(c) => Val::Int(c),
                None => Val::Null,
            },
        );
        m.insert(ustr("hostname"), Val::String(self.hostname.clone()));
        m.insert(ustr("username"), Val::String(self.username.clone()));
        m.insert(ustr("session_id"), Val::String(self.session_id.clone()));
        Val::Map(m)
    }
}

pub fn get_db_path() -> PathBuf {
    if let Ok(test_path) = std::env::var("FSH_TEST_DB_PATH") {
        return PathBuf::from(test_path);
    }
    fshell_engine::config_dir()
        .map(|d| d.join("history.db"))
        .unwrap_or_else(|| PathBuf::from(".fsh_history.db"))
}

pub fn get_hostname() -> String {
    if let Ok(h) = std::env::var("HOSTNAME") {
        return h;
    }
    let mut buf = [0u8; 256];
    let res = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if res == 0 {
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..len]).into_owned()
    } else {
        "unknown".to_string()
    }
}

#[derive(Clone)]
struct SqliteConnManager {
    path: PathBuf,
}

impl ManageConnection for SqliteConnManager {
    type Connection = Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        Connection::open(&self.path)
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        conn.execute_batch("SELECT 1")
    }

    fn has_broken(&self, _conn: &mut Self::Connection) -> bool {
        false
    }
}

use std::sync::Mutex;

static DB_POOL: Mutex<Option<r2d2::Pool<SqliteConnManager>>> = Mutex::new(None);
static RECENT_COMMANDS_CACHE: Mutex<Option<std::collections::HashSet<String>>> = Mutex::new(None);

pub fn get_recent_commands_cached() -> std::collections::HashSet<String> {
    {
        if let Ok(guard) = RECENT_COMMANDS_CACHE.lock()
            && let Some(ref cached) = *guard
        {
            return cached.clone();
        }
    }

    let entries = query_history(Some(50), None, None, None, None, None).unwrap_or_default();
    let set: std::collections::HashSet<String> = entries.into_iter().map(|e| e.command).collect();

    if let Ok(mut guard) = RECENT_COMMANDS_CACHE.lock() {
        *guard = Some(set.clone());
    }
    set
}

fn get_pool() -> r2d2::Pool<SqliteConnManager> {
    let mut guard = DB_POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(ref pool) = *guard {
        pool.clone()
    } else {
        let path = get_db_path();
        let manager = SqliteConnManager { path };
        let pool = r2d2::Pool::builder()
            .max_size(4)
            .connection_timeout(Duration::from_secs(5))
            .build(manager)
            .unwrap_or_else(|e| {
                eprintln!("Failed to create SQLite connection pool: {e}");
                std::process::abort();
            });
        let cloned = pool.clone();
        *guard = Some(pool);
        cloned
    }
}

pub fn clear_connection_cache() {
    let mut cache = DB_POOL.lock().unwrap_or_else(|e| e.into_inner());
    *cache = None;
}

/// Adapter wrapping our SQLite history to satisfy `reedline::History`.
/// Used by `FshellHinter` to provide history-based inline hints (ghost text).
pub struct SqliteHistoryAdapter;

impl reedline::History for SqliteHistoryAdapter {
    fn save(&mut self, _h: ReedlineHistoryItem) -> reedline::Result<ReedlineHistoryItem> {
        // Hints are read-only; save is not used by the hinter.
        unimplemented!("SqliteHistoryAdapter::save is not implemented")
    }

    fn load(&self, _id: reedline::HistoryItemId) -> reedline::Result<ReedlineHistoryItem> {
        unimplemented!("SqliteHistoryAdapter::load is not implemented")
    }

    fn count(&self, _query: SearchQuery) -> reedline::Result<i64> {
        Ok(0)
    }

    fn search(&self, query: SearchQuery) -> reedline::Result<Vec<ReedlineHistoryItem>> {
        // We only support prefix search for hints.
        let prefix = match query.filter.command_line {
            Some(CommandLineSearch::Prefix(p)) => p,
            _ => return Ok(Vec::new()),
        };

        let limit = query.limit.unwrap_or(1);

        with_db_conn(|conn| {
            let mut stmt = conn
                .prepare(
                    "SELECT command FROM history\n                     WHERE command LIKE ?1\n                     ORDER BY timestamp DESC\n                     LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let pattern = format!("{}%", prefix);
            let rows = stmt
                .query_map(
                    params![pattern, limit],
                    |row| {
                        let command_line: String = row.get(0)?;
                        Ok(ReedlineHistoryItem {
                            id: None,
                            start_timestamp: None,
                            command_line,
                            session_id: None,
                            hostname: None,
                            cwd: None,
                            duration: None,
                            exit_status: None,
                            more_info: None,
                        })
                    },
                )
                .map_err(|e| e.to_string())?;
            let mut results = Vec::new();
            for row in rows {
                results.push(row.map_err(|e| e.to_string())?);
            }
            Ok(results)
        })
        .map_err(|_| reedline::ReedlineError(reedline::ReedlineErrorVariants::OtherHistoryError("sqlite history search failed")))
    }

    fn update(
        &mut self,
        _id: reedline::HistoryItemId,
        _updater: &dyn Fn(ReedlineHistoryItem) -> ReedlineHistoryItem,
    ) -> reedline::Result<()> {
        unimplemented!("SqliteHistoryAdapter::update is not implemented")
    }

    fn clear(&mut self) -> reedline::Result<()> {
        unimplemented!("SqliteHistoryAdapter::clear is not implemented")
    }

    fn delete(&mut self, _id: reedline::HistoryItemId) -> reedline::Result<()> {
        unimplemented!("SqliteHistoryAdapter::delete is not implemented")
    }

    fn sync(&mut self) -> std::io::Result<()> {
        Ok(())
    }

    fn session(&self) -> Option<HistorySessionId> {
        None
    }
}

pub fn with_db_conn<F, R>(f: F) -> Result<R, String>
where
    F: FnOnce(&Connection) -> Result<R, String>,
{
    let pool = get_pool();
    let conn = pool.get().map_err(|e| e.to_string())?;
    f(&conn)
}

pub fn init_db() -> Result<(), String> {
    let pool = get_pool();
    let conn = pool.get().map_err(|e| e.to_string())?;
    let _ = conn.execute("PRAGMA journal_mode=WAL", []);
    let _ = conn.execute("PRAGMA synchronous=NORMAL", []);
    conn.execute(
        "CREATE TABLE IF NOT EXISTS history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            command TEXT NOT NULL,
            cwd TEXT NOT NULL,
            timestamp INTEGER NOT NULL, -- milliseconds since epoch
            duration_ms INTEGER NOT NULL,
            exit_code INTEGER,
            hostname TEXT NOT NULL,
            username TEXT NOT NULL,
            session_id TEXT NOT NULL
        )",
        [],
    )
    .map_err(|e| e.to_string())?;

    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_history_command ON history(command)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_history_exit_code_command ON history(exit_code, command)",
        [],
    );
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_history_timestamp ON history(timestamp DESC)",
        [],
    );

    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn log_command(
    command: &str,
    cwd: &str,
    timestamp_ms: i64,
    duration_ms: i64,
    exit_code: Option<i64>,
    hostname: &str,
    username: &str,
    session_id: &str,
) -> Result<i64, String> {
    if let Ok(mut guard) = RECENT_COMMANDS_CACHE.lock() {
        *guard = None;
    }
    with_db_conn(|conn| {
        conn.execute(
            "INSERT INTO history (command, cwd, timestamp, duration_ms, exit_code, hostname, username, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                command,
                cwd,
                timestamp_ms,
                duration_ms,
                exit_code,
                hostname,
                username,
                session_id,
            ],
        ).map_err(|e| e.to_string())?;
        let id = conn.last_insert_rowid();
        Ok(id)
    })
}

/// Update an existing history entry with the actual duration and exit code.
/// Called after command execution completes to overwrite the placeholder values
/// that were set before execution (to protect commands like `reload -bd` that
/// replace the process before any async logging can run).
pub fn update_history_entry(id: i64, duration_ms: i64, exit_code: i64) -> Result<(), String> {
    with_db_conn(|conn| {
        conn.execute(
            "UPDATE history SET duration_ms = ?1, exit_code = ?2 WHERE id = ?3",
            params![duration_ms, exit_code, id],
        )
        .map_err(|e| e.to_string())?;
        Ok(())
    })
}

/// Delete a single history entry by its ID.
pub fn delete_history_entry(id: i64) -> Result<(), String> {
    with_db_conn(|conn| {
        conn.execute("DELETE FROM history WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        Ok(())
    })
}

pub fn query_history(
    limit: Option<usize>,
    filter_command: Option<&str>,
    filter_cwd: Option<&str>,
    filter_session: Option<&str>,
    filter_host: Option<&str>,
    filter_exit: Option<i64>,
) -> Result<Vec<HistoryEntry>, String> {
    with_db_conn(|conn| {
        let mut query = "SELECT id, command, cwd, timestamp, duration_ms, exit_code, hostname, username, session_id FROM history WHERE 1=1".to_string();
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(cmd) = filter_command {
            query.push_str(" AND command LIKE ?");
            params_vec.push(Box::new(format!("%{}%", cmd)));
        }
        if let Some(cwd) = filter_cwd {
            query.push_str(" AND cwd = ?");
            params_vec.push(Box::new(cwd.to_string()));
        }
        if let Some(sess) = filter_session {
            query.push_str(" AND session_id = ?");
            params_vec.push(Box::new(sess.to_string()));
        }
        if let Some(host) = filter_host {
            query.push_str(" AND hostname = ?");
            params_vec.push(Box::new(host.to_string()));
        }
        if let Some(exit) = filter_exit {
            query.push_str(" AND exit_code = ?");
            params_vec.push(Box::new(exit));
        }

        query.push_str(" ORDER BY timestamp DESC");

        if let Some(lim) = limit {
            query.push_str(&format!(" LIMIT {}", lim));
        }

        let mut stmt = conn.prepare(&query).map_err(|e| e.to_string())?;
        let params_slice: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();

        let rows = stmt
            .query_map(&params_slice[..], |row| {
                Ok(HistoryEntry {
                    id: row.get(0)?,
                    command: row.get(1)?,
                    cwd: row.get(2)?,
                    timestamp_ms: row.get(3)?,
                    duration_ms: row.get(4)?,
                    exit_code: row.get(5)?,
                    hostname: row.get(6)?,
                    username: row.get(7)?,
                    session_id: row.get(8)?,
                })
            })
            .map_err(|e| e.to_string())?;

        let mut entries = Vec::new();
        for r in rows {
            entries.push(r.map_err(|e| e.to_string())?);
        }

        Ok(entries)
    })
}

#[derive(Debug, Clone)]
pub struct HistoryStats {
    pub total_commands: i64,
    pub unique_commands: i64,
    pub success_rate: f64,
    pub top_commands: Vec<(String, i64)>,
}

/// Query frequently used commands matching a prefix, for autocomplete suggestions.
pub fn query_frequent_by_prefix(prefix: &str, limit: usize) -> Result<Vec<(String, i64)>, String> {
    with_db_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT command, COUNT(*) as freq \
                 FROM history \
                 WHERE command LIKE ?1 \
                 GROUP BY command \
                 ORDER BY freq DESC, MAX(timestamp) DESC \
                 LIMIT ?2",
            )
            .map_err(|e| e.to_string())?;
        let pattern = format!("{}%", prefix);
        let rows = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        let mut results = Vec::new();
        for r in rows {
            results.push(r.map_err(|e| e.to_string())?);
        }
        Ok(results)
    })
}

/// Query commands matching a prefix via SQL, returning recent entries first.
/// Used by FTUI arrow-up navigation to find commands starting with the typed prefix.
/// When `limit` is 0, all matching entries are returned (no LIMIT clause).
pub fn query_history_prefix(prefix: &str, limit: usize) -> Result<Vec<String>, String> {
    with_db_conn(|conn| {
        let mut stmt = if limit > 0 {
            conn.prepare(
                "SELECT command FROM history \
                 WHERE command LIKE ?1 \
                 ORDER BY id DESC \
                 LIMIT ?2",
            )
            .map_err(|e| e.to_string())?
        } else {
            conn.prepare(
                "SELECT command FROM history \
                 WHERE command LIKE ?1 \
                 ORDER BY id DESC",
            )
            .map_err(|e| e.to_string())?
        };
        let pattern = format!("{}%", prefix);
        let mut result = Vec::new();
        if limit > 0 {
            let rows = stmt
                .query_map(params![pattern, limit as i64], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(|e| e.to_string())?;
            for row in rows {
                if let Ok(cmd) = row
                    && result.last() != Some(&cmd)
                {
                    result.push(cmd);
                }
            }
        } else {
            let rows = stmt
                .query_map(params![pattern], |row| row.get::<_, String>(0))
                .map_err(|e| e.to_string())?;
            for row in rows {
                if let Ok(cmd) = row
                    && result.last() != Some(&cmd)
                {
                    result.push(cmd);
                }
            }
        }
        Ok(result)
    })
}

/// Query the most recent command matching a prefix, for inline hints (ghost text).
/// Returns (command, timestamp_ms) of the most recent match.
pub fn query_recent_by_prefix(prefix: &str) -> Result<Option<(String, i64)>, String> {
    with_db_conn(|conn| {
        let mut stmt = conn
            .prepare(
                "SELECT command, timestamp\n                 FROM history\n                 WHERE command LIKE ?1\n                 ORDER BY timestamp DESC\n                 LIMIT 1",
            )
            .map_err(|e| e.to_string())?;
        let pattern = format!("{}%", prefix);
        let mut rows = stmt
            .query_map(params![pattern], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        match rows.next() {
            Some(Ok(row)) => Ok(Some(row)),
            _ => Ok(None),
        }
    })
}

pub fn get_stats() -> Result<HistoryStats, String> {
    with_db_conn(|conn| {
        let total_commands: i64 = conn
            .query_row("SELECT COUNT(*) FROM history", [], |row| row.get(0))
            .unwrap_or(0);

        let unique_commands: i64 = conn
            .query_row("SELECT COUNT(DISTINCT command) FROM history", [], |row| {
                row.get(0)
            })
            .unwrap_or(0);

        let successful: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM history WHERE exit_code = 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let success_rate = if total_commands > 0 {
            (successful as f64 / total_commands as f64) * 100.0
        } else {
            0.0
        };

        let mut stmt = conn.prepare("SELECT command, COUNT(*) as cnt FROM history GROUP BY command ORDER BY cnt DESC LIMIT 5")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;

        let mut top_commands = Vec::new();
        for r in rows {
            top_commands.push(r.map_err(|e| e.to_string())?);
        }

        Ok(HistoryStats {
            total_commands,
            unique_commands,
            success_rate,
            top_commands,
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fshell_core::{remove_var, set_var};

    #[test]
    fn test_sqlite_history_flow() {
        let _lock = TEST_DB_LOCK.blocking_lock();
        clear_connection_cache();
        let temp_dir = std::env::temp_dir();
        let test_db_file = temp_dir.join(format!(
            "fshell_test_hist_{}.db",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));

        set_var("FSH_TEST_DB_PATH", &test_db_file.to_string_lossy());

        // 1. Init
        init_db().unwrap();

        // 2. Log some commands
        log_command(
            "cargo build",
            "/workspace",
            1000,
            150,
            Some(0),
            "myhost",
            "myuser",
            "sess1",
        )
        .unwrap();
        log_command(
            "cargo test",
            "/workspace",
            2000,
            320,
            Some(0),
            "myhost",
            "myuser",
            "sess1",
        )
        .unwrap();
        log_command(
            "invalid_cmd",
            "/workspace",
            3000,
            50,
            Some(1),
            "myhost",
            "myuser",
            "sess1",
        )
        .unwrap();
        log_command(
            "cd ..",
            "/home",
            4000,
            10,
            Some(0),
            "otherhost",
            "myuser",
            "sess2",
        )
        .unwrap();

        // 3. Query all
        let all = query_history(None, None, None, None, None, None).unwrap();
        assert_eq!(all.len(), 4);

        // Ordered by timestamp DESC
        assert_eq!(all[0].command, "cd ..");
        assert_eq!(all[1].command, "invalid_cmd");

        // 4. Query with command filter
        let cargo_cmds = query_history(None, Some("cargo"), None, None, None, None).unwrap();
        assert_eq!(cargo_cmds.len(), 2);

        // 5. Query with exit filter
        let successful_cmds = query_history(None, None, None, None, None, Some(0)).unwrap();
        assert_eq!(successful_cmds.len(), 3);

        // 6. Query with session filter
        let sess2_cmds = query_history(None, None, None, Some("sess2"), None, None).unwrap();
        assert_eq!(sess2_cmds.len(), 1);
        assert_eq!(sess2_cmds[0].command, "cd ..");

        // 7. Stats
        let stats = get_stats().unwrap();
        assert_eq!(stats.total_commands, 4);
        assert_eq!(stats.unique_commands, 4);
        assert_eq!(stats.success_rate, 75.0);
        assert_eq!(stats.top_commands.len(), 4);

        // Clean up
        clear_connection_cache();
        let _ = std::fs::remove_file(test_db_file);
        remove_var("FSH_TEST_DB_PATH");
    }
}
