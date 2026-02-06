//! SQLite database for the wiki index.

use rusqlite::{Connection, params};
use std::sync::Mutex;

use tracing::info;

/// SQLite-backed wiki index.
pub struct WikiDb {
    conn: Mutex<Connection>,
}

/// A wiki entry row from the database.
#[derive(Debug, Clone)]
pub struct WikiEntryRow {
    pub did: String,
    pub rkey: String,
    pub slug: String,
    pub title: String,
    pub summary: Option<String>,
    pub content: String,
    pub status: String,
    pub aliases: String,
    pub tags: String,
    pub created_at: String,
    pub last_updated: String,
}

/// A wiki link row from the database.
#[derive(Debug, Clone)]
pub struct WikiLinkRow {
    pub did: String,
    pub rkey: String,
    pub source_uri: String,
    pub target_uri: String,
    pub link_type: String,
    pub source_anchor: Option<String>,
    pub target_anchor: Option<String>,
    pub context: Option<String>,
    pub created_at: String,
}

impl WikiDb {
    /// Open or create the SQLite database.
    pub fn open(path: &str) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;

        // Create tables
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS wiki_entries (
                did TEXT NOT NULL,
                rkey TEXT NOT NULL,
                slug TEXT NOT NULL,
                title TEXT NOT NULL,
                summary TEXT,
                content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'stable',
                aliases TEXT DEFAULT '[]',
                tags TEXT DEFAULT '[]',
                created_at TEXT NOT NULL,
                last_updated TEXT NOT NULL,
                indexed_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (did, rkey)
            );
            CREATE INDEX IF NOT EXISTS idx_entries_slug ON wiki_entries(did, slug);
            CREATE INDEX IF NOT EXISTS idx_entries_status ON wiki_entries(status);

            CREATE TABLE IF NOT EXISTS wiki_links (
                did TEXT NOT NULL,
                rkey TEXT NOT NULL,
                source_uri TEXT NOT NULL,
                target_uri TEXT NOT NULL,
                link_type TEXT NOT NULL,
                source_anchor TEXT,
                target_anchor TEXT,
                context TEXT,
                created_at TEXT NOT NULL,
                indexed_at TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (did, rkey)
            );
            CREATE INDEX IF NOT EXISTS idx_links_source ON wiki_links(source_uri);
            CREATE INDEX IF NOT EXISTS idx_links_target ON wiki_links(target_uri);
            CREATE INDEX IF NOT EXISTS idx_links_type ON wiki_links(link_type);

            CREATE TABLE IF NOT EXISTS did_handles (
                did TEXT PRIMARY KEY,
                handle TEXT NOT NULL,
                resolved_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS state (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;

        info!(path = %path, "wiki database initialized");

        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    // =========================================================================
    // Wiki entries
    // =========================================================================

    /// Upsert a wiki entry.
    pub fn upsert_entry(
        &self,
        did: &str,
        rkey: &str,
        entry: &winter_atproto::WikiEntry,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO wiki_entries
             (did, rkey, slug, title, summary, content, status, aliases, tags, created_at, last_updated, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, datetime('now'))",
            params![
                did,
                rkey,
                entry.slug,
                entry.title,
                entry.summary,
                entry.content,
                entry.status,
                serde_json::to_string(&entry.aliases).unwrap_or_default(),
                serde_json::to_string(&entry.tags).unwrap_or_default(),
                entry.created_at.to_rfc3339(),
                entry.last_updated.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Delete a wiki entry.
    pub fn delete_entry(&self, did: &str, rkey: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM wiki_entries WHERE did = ?1 AND rkey = ?2",
            params![did, rkey],
        )?;
        Ok(())
    }

    /// Delete all entries and links for a DID (used before re-backfill).
    pub fn clear_did(&self, did: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM wiki_entries WHERE did = ?1", params![did])?;
        conn.execute("DELETE FROM wiki_links WHERE did = ?1", params![did])?;
        Ok(())
    }

    /// Get a wiki entry by DID and slug.
    pub fn get_entry_by_slug(&self, did: &str, slug: &str) -> Result<Option<WikiEntryRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT did, rkey, slug, title, summary, content, status, aliases, tags, created_at, last_updated
             FROM wiki_entries WHERE did = ?1 AND (slug = ?2 OR aliases LIKE ?3) LIMIT 1",
        )?;

        let alias_pattern = format!("%\"{}%", slug);
        let result = stmt
            .query_row(params![did, slug, alias_pattern], |row| {
                Ok(WikiEntryRow {
                    did: row.get(0)?,
                    rkey: row.get(1)?,
                    slug: row.get(2)?,
                    title: row.get(3)?,
                    summary: row.get(4)?,
                    content: row.get(5)?,
                    status: row.get(6)?,
                    aliases: row.get(7)?,
                    tags: row.get(8)?,
                    created_at: row.get(9)?,
                    last_updated: row.get(10)?,
                })
            })
            .optional()?;

        Ok(result)
    }

    /// List all entries for a user.
    pub fn list_entries_by_did(&self, did: &str) -> Result<Vec<WikiEntryRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT did, rkey, slug, title, summary, content, status, aliases, tags, created_at, last_updated
             FROM wiki_entries WHERE did = ?1 ORDER BY last_updated DESC",
        )?;

        let rows = stmt
            .query_map(params![did], |row| {
                Ok(WikiEntryRow {
                    did: row.get(0)?,
                    rkey: row.get(1)?,
                    slug: row.get(2)?,
                    title: row.get(3)?,
                    summary: row.get(4)?,
                    content: row.get(5)?,
                    status: row.get(6)?,
                    aliases: row.get(7)?,
                    tags: row.get(8)?,
                    created_at: row.get(9)?,
                    last_updated: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Search entries globally.
    pub fn search_entries(&self, query: &str, limit: usize) -> Result<Vec<WikiEntryRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT did, rkey, slug, title, summary, content, status, aliases, tags, created_at, last_updated
             FROM wiki_entries
             WHERE title LIKE ?1 OR slug LIKE ?1 OR content LIKE ?1
             ORDER BY last_updated DESC
             LIMIT ?2",
        )?;

        let rows = stmt
            .query_map(params![pattern, limit as i64], |row| {
                Ok(WikiEntryRow {
                    did: row.get(0)?,
                    rkey: row.get(1)?,
                    slug: row.get(2)?,
                    title: row.get(3)?,
                    summary: row.get(4)?,
                    content: row.get(5)?,
                    status: row.get(6)?,
                    aliases: row.get(7)?,
                    tags: row.get(8)?,
                    created_at: row.get(9)?,
                    last_updated: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    /// Get recent entries across all users.
    pub fn recent_entries(&self, limit: usize) -> Result<Vec<WikiEntryRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT did, rkey, slug, title, summary, content, status, aliases, tags, created_at, last_updated
             FROM wiki_entries
             WHERE status != 'draft'
             ORDER BY last_updated DESC
             LIMIT ?1",
        )?;

        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(WikiEntryRow {
                    did: row.get(0)?,
                    rkey: row.get(1)?,
                    slug: row.get(2)?,
                    title: row.get(3)?,
                    summary: row.get(4)?,
                    content: row.get(5)?,
                    status: row.get(6)?,
                    aliases: row.get(7)?,
                    tags: row.get(8)?,
                    created_at: row.get(9)?,
                    last_updated: row.get(10)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    // =========================================================================
    // Wiki links
    // =========================================================================

    /// Insert a wiki link.
    pub fn insert_link(
        &self,
        did: &str,
        rkey: &str,
        link: &winter_atproto::WikiLink,
    ) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO wiki_links
             (did, rkey, source_uri, target_uri, link_type, source_anchor, target_anchor, context, created_at, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, datetime('now'))",
            params![
                did,
                rkey,
                link.source,
                link.target,
                link.link_type,
                link.source_anchor,
                link.target_anchor,
                link.context,
                link.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Delete a wiki link.
    pub fn delete_link(&self, did: &str, rkey: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM wiki_links WHERE did = ?1 AND rkey = ?2",
            params![did, rkey],
        )?;
        Ok(())
    }

    /// Get backlinks targeting a specific entry URI.
    pub fn get_backlinks(&self, target_uri: &str) -> Result<Vec<WikiLinkRow>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT did, rkey, source_uri, target_uri, link_type, source_anchor, target_anchor, context, created_at
             FROM wiki_links WHERE target_uri = ?1",
        )?;

        let rows = stmt
            .query_map(params![target_uri], |row| {
                Ok(WikiLinkRow {
                    did: row.get(0)?,
                    rkey: row.get(1)?,
                    source_uri: row.get(2)?,
                    target_uri: row.get(3)?,
                    link_type: row.get(4)?,
                    source_anchor: row.get(5)?,
                    target_anchor: row.get(6)?,
                    context: row.get(7)?,
                    created_at: row.get(8)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(rows)
    }

    // =========================================================================
    // State management
    // =========================================================================

    /// Get the firehose cursor.
    pub fn get_cursor(&self) -> Result<Option<i64>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        let result = conn
            .query_row(
                "SELECT value FROM state WHERE key = 'cursor'",
                [],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .and_then(|s| s.parse().ok());

        Ok(result)
    }

    /// Set the firehose cursor.
    pub fn set_cursor(&self, cursor: i64) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO state (key, value) VALUES ('cursor', ?1)",
            params![cursor.to_string()],
        )?;
        Ok(())
    }

    // =========================================================================
    // Handle resolution cache
    // =========================================================================

    /// Get a cached handle for a DID.
    pub fn get_handle(&self, did: &str) -> Result<Option<String>, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT handle FROM did_handles WHERE did = ?1",
            params![did],
            |row| row.get(0),
        )
        .optional()
    }

    /// Cache a DID -> handle mapping.
    pub fn set_handle(&self, did: &str, handle: &str) -> Result<(), rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO did_handles (did, handle, resolved_at) VALUES (?1, ?2, datetime('now'))",
            params![did, handle],
        )?;
        Ok(())
    }

    /// Get entry count.
    pub fn entry_count(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row("SELECT COUNT(*) FROM wiki_entries", [], |row| {
            row.get::<_, usize>(0)
        })
    }

    /// Get distinct author count.
    pub fn author_count(&self) -> Result<usize, rusqlite::Error> {
        let conn = self.conn.lock().unwrap();
        conn.query_row(
            "SELECT COUNT(DISTINCT did) FROM wiki_entries",
            [],
            |row| row.get::<_, usize>(0),
        )
    }
}

/// Extension trait for optional query results.
trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>, rusqlite::Error> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
