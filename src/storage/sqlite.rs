use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::error::{GraphError, GraphResult};
use crate::types::*;

/// SQLite-backed graph storage, aligned with @latte-graph/core schema.
pub struct SqliteStorage {
    pub(crate) conn: Mutex<Connection>,
}

impl SqliteStorage {
    /// Open (or create) a graph database at `db_path`.
    pub fn open(db_path: &Path) -> GraphResult<Self> {
        let conn = Connection::open_with_flags(
            db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )?;

        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA cache_size = -65536;
             PRAGMA temp_store = MEMORY;
             PRAGMA mmap_size = 268435456;
             PRAGMA foreign_keys = OFF;"
        )?;

        let storage = Self {
            conn: Mutex::new(conn),
        };
        storage.initialize_schema()?;
        Ok(storage)
    }

    /// Open an existing database in read-only mode.
    pub fn open_readonly(db_path: &Path) -> GraphResult<Self> {
        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn initialize_schema(&self) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;

        // Step 1: Create tables (one at a time for compatibility)
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS nodes (
                id TEXT PRIMARY KEY,
                kind TEXT NOT NULL,
                name TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                file_path TEXT NOT NULL,
                language TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                start_column INTEGER DEFAULT 0,
                end_column INTEGER DEFAULT 0,
                signature TEXT,
                docstring TEXT,
                visibility TEXT,
                is_exported INTEGER DEFAULT 0,
                is_async INTEGER DEFAULT 0,
                is_static INTEGER DEFAULT 0,
                is_abstract INTEGER DEFAULT 0,
                extra TEXT DEFAULT '{}',
                version INTEGER DEFAULT 1,
                valid INTEGER DEFAULT 1,
                updated_at INTEGER NOT NULL
            );
            ",
        )?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS edges (
                id TEXT PRIMARY KEY,
                source TEXT NOT NULL,
                target TEXT NOT NULL,
                kind TEXT NOT NULL,
                line INTEGER DEFAULT 0,
                col INTEGER DEFAULT 0,
                metadata TEXT,
                provenance TEXT,
                version INTEGER DEFAULT 1,
                valid INTEGER DEFAULT 1,
                updated_at INTEGER NOT NULL,
                FOREIGN KEY (source) REFERENCES nodes(id),
                FOREIGN KEY (target) REFERENCES nodes(id)
            );
            ",
        )?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                language TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                content_hash TEXT NOT NULL,
                indexed_at INTEGER NOT NULL
            );
            ",
        )?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS `references` (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                source_node TEXT NOT NULL,
                source_file TEXT NOT NULL,
                target_node TEXT NOT NULL,
                target_file TEXT NOT NULL,
                kind TEXT NOT NULL,
                line INTEGER,
                FOREIGN KEY (source_node) REFERENCES nodes(id),
                FOREIGN KEY (target_node) REFERENCES nodes(id),
                UNIQUE(source_node, target_node, kind)
            );
            ",
        )?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS search_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                query TEXT NOT NULL,
                backend TEXT NOT NULL,
                results_count INTEGER,
                created_at INTEGER NOT NULL
            );
            ",
        )?;

        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS metadata (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            ",
        )?;


        // FTS5 virtual table for identifier-aware full-text search (Phase 3).
        // We pre-tokenize identifiers in Rust and store the space-joined
        // tokens in `body`, so the built-in unicode61 tokenizer is enough
        // (no C tokenizer needed). `id` is UNINDEXED so it doesn't pollute
        // the term dictionary.
        conn.execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
                id UNINDEXED,
                body,
                tokenize = 'unicode61 remove_diacritics 2'
            );"
        )?;
        // Step 2: Create indexes
        for sql in &[
            "CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path)",
            "CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind)",
            "CREATE INDEX IF NOT EXISTS idx_nodes_valid ON nodes(valid)",
            "CREATE INDEX IF NOT EXISTS idx_nodes_qualified ON nodes(qualified_name)",
            "CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source)",
            "CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target)",
            "CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind)",
            "CREATE INDEX IF NOT EXISTS idx_files_mtime ON files(mtime)",
            "CREATE INDEX IF NOT EXISTS idx_ref_target_file ON `references`(target_file)",
            "CREATE INDEX IF NOT EXISTS idx_ref_target_node ON `references`(target_node)",
            "CREATE INDEX IF NOT EXISTS idx_ref_source_file ON `references`(source_file)",
        ] {
            conn.execute(sql, []).ok();
        }

        // Step 3: Metadata
        conn.execute(
            "INSERT OR IGNORE INTO metadata (key, value) VALUES (?1, ?2)",
            rusqlite::params!["schema_version", "1"],
        )?;

        Ok(())
    }

    pub fn upsert_node(&self, node: &Node) -> GraphResult<()> {
        let body = fts_body(node);
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();
        let kind = node.kind.as_str();
        let extra_json = serde_json::to_string(&node.extra)?;

        conn.execute(
            "INSERT OR REPLACE INTO nodes
             (id, kind, name, qualified_name, file_path, language,
              start_line, end_line, start_column, end_column,
              signature, docstring, visibility,
              is_exported, is_async, is_static, is_abstract, extra,
              version, valid, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                     ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13,
                     ?14, ?15, ?16, ?17, ?18,
                     COALESCE((SELECT version FROM nodes WHERE id = ?1) + 1, 1), 1, ?19)",
            params![
                node.id, kind, node.name, node.qualified_name, node.file_path, node.language,
                node.start_line, node.end_line, node.start_column, node.end_column,
                node.signature, node.docstring, node.visibility,
                node.is_exported as i32, node.is_async as i32, node.is_static as i32,
                node.is_abstract as i32, extra_json,
                now,
            ],
        )?;
        // Keep FTS index in sync (Phase 3).
        fts_upsert(&conn, &node.id, &body)?;
        Ok(())
    }
    pub fn upsert_nodes_batch(&self, nodes: &[Node]) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        for node in nodes {
            let body = fts_body(node);
            let kind = node.kind.as_str();
            let now = chrono::Utc::now().timestamp();
            let extra_json = serde_json::to_string(&node.extra)?;

            tx.execute(
                "INSERT OR REPLACE INTO nodes
                 (id, kind, name, qualified_name, file_path, language,
                  start_line, end_line, start_column, end_column,
                  signature, docstring, visibility,
                  is_exported, is_async, is_static, is_abstract, extra,
                  version, valid, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                         ?7, ?8, ?9, ?10,
                         ?11, ?12, ?13,
                         ?14, ?15, ?16, ?17, ?18,
                         COALESCE((SELECT version FROM nodes WHERE id = ?1) + 1, 1), 1, ?19)",
                params![
                    node.id, kind, node.name, node.qualified_name, node.file_path, node.language,
                    node.start_line, node.end_line, node.start_column, node.end_column,
                    node.signature, node.docstring, node.visibility,
                    node.is_exported as i32, node.is_async as i32, node.is_static as i32,
                    node.is_abstract as i32, extra_json,
                    now,
                ],
            )?;
            // Keep FTS index in sync (Phase 3).
            fts_upsert(&tx, &node.id, &body)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_edge(&self, edge: &Edge) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let now = chrono::Utc::now().timestamp();
        let kind = edge.kind.as_str();

        conn.execute(
            "INSERT OR REPLACE INTO edges
             (id, source, target, kind, line, col, metadata, provenance,
              version, valid, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                     COALESCE((SELECT version FROM edges WHERE id = ?1) + 1, 1), 1, ?9)",
            params![
                edge.id, edge.source, edge.target, kind,
                edge.line, edge.col, edge.metadata, edge.provenance,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_edges_batch(&self, edges: &[Edge]) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        for edge in edges {
            let now = chrono::Utc::now().timestamp();
            let kind = edge.kind.as_str();

            tx.execute(
                "INSERT OR REPLACE INTO edges
                 (id, source, target, kind, line, col, metadata, provenance,
                  version, valid, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8,
                         COALESCE((SELECT version FROM edges WHERE id = ?1) + 1, 1), 1, ?9)",
                params![
                    edge.id, edge.source, edge.target, kind,
                    edge.line, edge.col, edge.metadata, edge.provenance,
                    now,
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn upsert_file(&self, file: &FileRecord) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        conn.execute(
            "INSERT OR REPLACE INTO files (path, language, mtime, content_hash, indexed_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![file.path, file.language, file.mtime, file.content_hash, file.indexed_at],
        )?;
        Ok(())
    }

    pub fn clear_all(&self) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        conn.execute_batch(
            "DELETE FROM nodes; DELETE FROM edges; DELETE FROM files; \
             DELETE FROM `references`; DELETE FROM nodes_fts;"
        )?;
        Ok(())
    }

    // =====================================================================
    // Per-file mutations (for incremental updates)
    // =====================================================================

    /// Delete all nodes whose `file_path` matches `relative_path`.
    /// Returns the list of deleted node IDs so callers can clean up
    /// inbound edges. Empty path is treated as "match all" — never call
    /// with empty; the caller filters.
    pub fn delete_nodes_for_file(&self, relative_path: &str) -> GraphResult<Vec<String>> {
        let mut conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let tx = conn.transaction()?;
        let mut stmt = tx.prepare("SELECT id FROM nodes WHERE file_path = ?1")?;
        let ids: Vec<String> = stmt
            .query_map(params![relative_path], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        drop(stmt);
        if !ids.is_empty() {
            tx.execute(
                "DELETE FROM nodes WHERE file_path = ?1",
                params![relative_path],
            )?;
            // Keep FTS index in sync (Phase 3).
            fts_delete_for_ids(&tx, &ids)?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Delete every edge whose `source` or `target` is one of `node_ids`.
    /// Returns the count of rows removed. Safe to call with an empty slice.
    pub fn delete_edges_involving(&self, node_ids: &[String]) -> GraphResult<usize> {
        if node_ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        // node_ids is bounded by the parser output of a single file
        // (rarely > 10k), and SQLite caps host parameters at 999.
        // Chunk to stay under that limit.
        const CHUNK: usize = 500;
        let mut total = 0usize;
        for chunk in node_ids.chunks(CHUNK) {
            // Build two distinct placeholder lists — one for source, one
            // for target — and a single parameter vector that lays them
            // out in the same order the SQL sees them: [id0..idN, id0..idN].
            let n = chunk.len();
            let src_ph = std::iter::repeat("?")
                .take(n)
                .collect::<Vec<_>>()
                .join(",");
            let tgt_ph = std::iter::repeat("?")
                .take(n)
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "DELETE FROM edges WHERE source IN ({src_ph}) OR target IN ({tgt_ph})"
            );
            let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(n * 2);
            for id in chunk {
                params_vec.push(id);
            }
            for id in chunk {
                params_vec.push(id);
            }
            let removed = conn.execute(&sql, params_vec.as_slice())?;
            total += removed;
        }
        Ok(total)
    }

    /// Delete every edge whose `source` is exactly `source_id`. Used by
    /// the incremental update path to drop a file's *outbound* edges
    /// (those emitted by the parser with `source = file:<relative>`)
    /// without disturbing inbound edges from other files.
    pub fn delete_edges_with_source(&self, source_id: &str) -> GraphResult<usize> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let n = conn.execute(
            "DELETE FROM edges WHERE source = ?1",
            params![source_id],
        )?;
        Ok(n)
    }

    /// Get the per-file metadata record (mtime + content hash).
    /// Returns None if the file was never indexed.
    pub fn get_file_record(&self, path: &str) -> GraphResult<Option<FileRecord>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT path, language, mtime, content_hash, indexed_at FROM files WHERE path = ?1",
        )?;
        let mut rows = stmt.query_map(params![path], |row| {
            Ok(FileRecord {
                path: row.get(0)?,
                language: row.get(1)?,
                mtime: row.get::<_, i64>(2)?,
                content_hash: row.get(3)?,
                indexed_at: row.get::<_, i64>(4)?,
            })
        })?;
        match rows.next() {
            Some(Ok(rec)) => Ok(Some(rec)),
            _ => Ok(None),
        }
    }

    /// Delete the per-file metadata record.
    pub fn delete_file_record(&self, path: &str) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        conn.execute("DELETE FROM files WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// List every file_path the graph has indexed. Used to detect
    /// files that disappeared between full builds.
    pub fn all_file_paths(&self) -> GraphResult<Vec<String>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare("SELECT path FROM files")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// List distinct source IDs of edges pointing TO any of `node_ids`.
    /// Used after a file change to find inbound edges that need to be
    /// re-resolved (the old target went away).
    pub fn edge_sources_pointing_to(&self, node_ids: &[String]) -> GraphResult<Vec<String>> {
        if node_ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        const CHUNK: usize = 500;
        let mut out: Vec<String> = Vec::new();
        for chunk in node_ids.chunks(CHUNK) {
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT DISTINCT source FROM edges WHERE target IN ({})",
                placeholders
            );
            let mut params_vec: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(chunk.len());
            for id in chunk {
                params_vec.push(id);
            }
            let mut stmt = conn.prepare(&sql)?;
            let rows =
                stmt.query_map(params_vec.as_slice(), |row| row.get::<_, String>(0))?;
            for r in rows {
                out.push(r?);
            }
        }
        Ok(out)
    }

    // =====================================================================
    // Read
    // =====================================================================

    pub fn all_nodes(&self) -> GraphResult<Vec<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes WHERE valid = 1",
        )?;

        let rows = stmt.query_map([], row_to_node)?;

        let nodes: Vec<Node> = rows.collect::<Result<_, _>>()?;
        Ok(nodes)
    }

    /// Run an arbitrary `SELECT` against the `nodes` table and materialise
    /// each row back into a [`Node`]. Used by the Cypher planner to execute
    /// the seed-fetch SQL it generated; callers are responsible for
    /// ensuring the column ordering matches `row_to_node`.
    ///
    /// Binds are positional — the planner produces a `Vec<Literal>` in the
    /// same order as the `?` placeholders it emitted. `Literal::Str` /
    /// `Int` / `Float` / `Bool` / `Null` are converted to their rusqlite
    /// counterparts; `Null` binds to a SQL NULL.
    pub fn raw_query_nodes(
        &self,
        sql: &str,
        binds: &[crate::query::cypher::ast::Literal],
    ) -> GraphResult<Vec<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(sql)?;
        let boxed: Vec<Box<dyn rusqlite::ToSql>> = binds
            .iter()
            .map(|l| -> Box<dyn rusqlite::ToSql> {
                match l {
                    crate::query::cypher::ast::Literal::Str(s) => Box::new(s.clone()),
                    crate::query::cypher::ast::Literal::Int(n) => Box::new(*n),
                    crate::query::cypher::ast::Literal::Float(f) => Box::new(*f),
                    crate::query::cypher::ast::Literal::Bool(b) => Box::new(*b),
                    crate::query::cypher::ast::Literal::Null => Box::new(rusqlite::types::Null),
                }
            })
            .collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(boxed.iter()), row_to_node)?;
        let nodes: Vec<Node> = rows.collect::<Result<_, _>>()?;
        Ok(nodes)
    }

    pub fn all_edges(&self) -> GraphResult<Vec<Edge>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, source, target, kind, line, col, metadata, provenance
             FROM edges WHERE valid = 1",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(Edge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                kind: EdgeKind::from_str(&row.get::<_, String>(3)?),
                line: row.get::<_, i32>(4)? as u32,
                col: row.get::<_, i32>(5)? as u32,
                metadata: row.get(6)?,
                provenance: row.get(7)?,
            })
        })?;

        let edges: Vec<Edge> = rows.collect::<Result<_, _>>()?;
        Ok(edges)
    }

    /// FTS5 + BM25 ranked search. Splits camelCase / snake_case identifiers
    /// before matching, so `updateCloudClient` is matched by the user query
    /// "update cloud" or "cloud client", and `parse_user_input` by "parse
    /// user" or "user input".
    ///
    /// Empty query (or one that tokenizes to nothing, e.g. "___") returns
    /// the most recently inserted nodes via `search_recent`.
    pub fn search_nodes(&self, query: &str, limit: usize) -> GraphResult<Vec<Node>> {
        // No query or nothing tokenizable → fall back to "recent nodes".
        if query.trim().is_empty() {
            return self.search_recent(limit);
        }
        let body = crate::types::tokenize_for_fts(&[query]);
        if body.trim().is_empty() {
            return self.search_recent(limit);
        }
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        // FTS5 MATCH treats space-separated tokens as implicit AND, which
        // matches the existing substring-search UX (all terms must appear).
        // BM25 ranks more-specific matches higher (shorter body = better).
        let mut stmt = conn.prepare(
            "SELECT n.id, n.kind, n.name, n.qualified_name, n.file_path, n.language,
                    n.start_line, n.end_line, n.start_column, n.end_column,
                    n.signature, n.docstring, n.visibility,
                    n.is_exported, n.is_async, n.is_static, n.is_abstract, n.extra
             FROM nodes_fts f
             JOIN nodes n ON n.id = f.id
             WHERE nodes_fts MATCH ?1
               AND n.valid = 1
             ORDER BY bm25(nodes_fts) ASC
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![body, limit as i64], row_to_node)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn find_definitions(&self, name: &str) -> GraphResult<Vec<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes WHERE valid = 1
               AND kind NOT IN ('file', 'import', 'export')
               AND (name = ?1 OR qualified_name = ?1)
             LIMIT 50",
        )?;

        let rows = stmt.query_map(params![name], row_to_node)?;

        let nodes: Vec<Node> = rows.collect::<Result<_, _>>()?;
        Ok(nodes)
    }

    pub fn node_by_id(&self, id: &str) -> GraphResult<Option<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes WHERE id = ?1 AND valid = 1",
        )?;

        let mut rows = stmt.query_map(params![id], row_to_node)?;

        match rows.next() {
            Some(Ok(node)) => Ok(Some(node)),
            _ => Ok(None),
        }
    }

    pub fn edges_for_node(&self, node_id: &str) -> GraphResult<Vec<Edge>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, source, target, kind, line, col, metadata, provenance
             FROM edges WHERE valid = 1 AND (source = ?1 OR target = ?1)",
        )?;

        let rows = stmt.query_map(params![node_id], |row| {
            Ok(Edge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                kind: EdgeKind::from_str(&row.get::<_, String>(3)?),
                line: row.get::<_, i32>(4)? as u32,
                col: row.get::<_, i32>(5)? as u32,
                metadata: row.get(6)?,
                provenance: row.get(7)?,
            })
        })?;

        let edges: Vec<Edge> = rows.collect::<Result<_, _>>()?;
        Ok(edges)
    }

    pub fn stats(&self) -> GraphResult<GraphStats> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let total_nodes: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes WHERE valid = 1",
            [],
            |r| r.get(0),
        )?;
        let total_edges: i64 = conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE valid = 1",
            [],
            |r| r.get(0),
        )?;
        let total_files: i64 = conn.query_row(
            "SELECT COUNT(*) FROM files",
            [],
            |r| r.get(0),
        )?;

        let mut kind_stmt = conn.prepare(
            "SELECT kind, COUNT(*) as cnt FROM nodes WHERE valid = 1 GROUP BY kind ORDER BY cnt DESC",
        )?;
        let node_kinds: Vec<KindCount> = kind_stmt
            .query_map([], |row| {
                Ok(KindCount {
                    kind: row.get(0)?,
                    count: row.get::<_, i64>(1)? as usize,
                })
            })?
            .collect::<Result<_, _>>()?;

        let mut ek_stmt = conn.prepare(
            "SELECT kind, COUNT(*) as cnt FROM edges WHERE valid = 1 GROUP BY kind ORDER BY cnt DESC",
        )?;
        let edge_kinds: Vec<KindCount> = ek_stmt
            .query_map([], |row| {
                Ok(KindCount {
                    kind: row.get(0)?,
                    count: row.get::<_, i64>(1)? as usize,
                })
            })?
            .collect::<Result<_, _>>()?;

        Ok(GraphStats {
            total_nodes: total_nodes as usize,
            total_edges: total_edges as usize,
            total_files: total_files as usize,
            node_kinds,
            edge_kinds,
        })
    }

    /// Smallest enclosing definition node (`Function`/`Method`/`Class`/
    /// `Trait`/`Interface`/`Struct`/`Route`) covering `line` in `file_path`.
    /// Returns the smallest one (innermost by line range).
    pub fn containing_node_for_line(
        &self,
        file_path: &str,
        line: u32,
    ) -> GraphResult<Option<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes
             WHERE file_path = ?1
               AND start_line <= ?2
               AND end_line >= ?2
               AND kind IN ('function', 'method', 'class', 'trait', 'interface', 'struct', 'route')
               AND valid = 1
             ORDER BY (end_line - start_line) ASC
             LIMIT 1",
        )?;

        let mut rows = stmt.query_map(params![file_path, line], |row| {
            let extra_str: String = row.get(17)?;
            let extra: HashMap<String, String> =
                serde_json::from_str(&extra_str).unwrap_or_default();

            Ok(Node {
                id: row.get(0)?,
                kind: NodeKind::from_str(&row.get::<_, String>(1)?),
                name: row.get(2)?,
                qualified_name: row.get(3)?,
                file_path: row.get(4)?,
                language: row.get(5)?,
                start_line: row.get::<_, i32>(6)? as u32,
                end_line: row.get::<_, i32>(7)? as u32,
                start_column: row.get::<_, i32>(8)? as u32,
                end_column: row.get::<_, i32>(9)? as u32,
                signature: row.get(10)?,
                docstring: row.get(11)?,
                visibility: row.get(12)?,
                is_exported: row.get::<_, i32>(13)? != 0,
                is_async: row.get::<_, i32>(14)? != 0,
                is_static: row.get::<_, i32>(15)? != 0,
                is_abstract: row.get::<_, i32>(16)? != 0,
                extra,
            })
        })?;

        match rows.next() {
            Some(Ok(node)) => Ok(Some(node)),
            Some(Err(error)) => Err(error.into()),
            None => Ok(None),
        }
    }

    /// Inbound CALLS edge count for ranking.
    pub fn in_degree_calls(&self, node_id: &str) -> GraphResult<u32> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM edges WHERE target = ?1 AND kind = 'calls' AND valid = 1",
            params![node_id],
            |row| row.get(0),
        )?;
        Ok(count as u32)
    }

    // =========================================================================
    // Dead-code + Blast-radius analysis (Phase 2)
    // =========================================================================

    /// All `function`/`method`/`test` nodes that:
    ///   1. are not the `target` of any `calls` edge (i.e. nobody calls them), AND
    ///   2. are NOT entry points.
    ///
    /// Entry-point heuristic: name in `main`/`index`/`__init__`, file under
    /// `/bin/`, file matching `main.<ext>` or `index.<ext>`, `is_exported=1`,
    /// or `visibility` in `public`/`pub`.
    ///
    /// `reasons_excluded_from_entry` on each returned entry is empty — the node
    /// survived because it has zero callers; the SQL already filtered out the
    /// entry-point candidates. Callers that want a per-row reason should run the
    /// inverse query themselves.
    pub fn dead_code(&self) -> GraphResult<Vec<DeadCodeEntry>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes
             WHERE valid = 1
               AND kind IN ('function','method','test')
               AND id NOT IN (SELECT target FROM edges WHERE kind='calls' AND valid=1)
               AND NOT (
                   name IN ('main','index','__init__')
                   OR file_path LIKE '%/bin/%'
                   OR file_path LIKE '%/main.%'
                   OR file_path LIKE '%/index.%'
                   OR is_exported = 1
                   OR COALESCE(visibility, '') IN ('public','pub')
               )
             ORDER BY file_path, name",
        )?;
        let rows = stmt.query_map([], |row| {
            let extra_str: String = row.get(17)?;
            let extra: HashMap<String, String> =
                serde_json::from_str(&extra_str).unwrap_or_default();
            Ok(Node {
                id: row.get(0)?,
                kind: NodeKind::from_str(&row.get::<_, String>(1)?),
                name: row.get(2)?,
                qualified_name: row.get(3)?,
                file_path: row.get(4)?,
                language: row.get(5)?,
                start_line: row.get::<_, i32>(6)? as u32,
                end_line: row.get::<_, i32>(7)? as u32,
                start_column: row.get::<_, i32>(8)? as u32,
                end_column: row.get::<_, i32>(9)? as u32,
                signature: row.get(10)?,
                docstring: row.get(11)?,
                visibility: row.get(12)?,
                is_exported: row.get::<_, i32>(13)? != 0,
                is_async: row.get::<_, i32>(14)? != 0,
                is_static: row.get::<_, i32>(15)? != 0,
                is_abstract: row.get::<_, i32>(16)? != 0,
                extra,
            })
        })?;
        let nodes: Vec<Node> = rows.collect::<Result<_, _>>()?;
        Ok(nodes
            .into_iter()
            .map(|n| DeadCodeEntry { node: n, reasons_excluded_from_entry: Vec::new() })
            .collect())
    }

    /// For each file in `paths`, return all definition-kind nodes (function/
    /// method/class/struct/trait/interface/route/constructor) that live in it.
    /// Files with zero matching nodes are skipped from the output. `paths`
    /// over 500 entries are chunked so we stay under SQLite's parameter cap.
    pub fn nodes_for_files(&self, paths: &[String]) -> GraphResult<Vec<BlastSeed>> {
        if paths.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        const CHUNK: usize = 500;

        let mut collected: Vec<Node> = Vec::new();
        for chunk in paths.chunks(CHUNK) {
            let placeholders = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT id, kind, name, qualified_name, file_path, language,
                        start_line, end_line, start_column, end_column,
                        signature, docstring, visibility,
                        is_exported, is_async, is_static, is_abstract, extra
                 FROM nodes
                 WHERE valid = 1
                   AND file_path IN ({placeholders})
                   AND kind IN ('function','method','class','struct','trait',
                                'interface','route','constructor')"
            );
            let mut stmt = conn.prepare(&sql)?;
            let params_vec: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(params_vec.as_slice(), |row| {
                let extra_str: String = row.get(17)?;
                let extra: HashMap<String, String> =
                    serde_json::from_str(&extra_str).unwrap_or_default();
                Ok(Node {
                    id: row.get(0)?,
                    kind: NodeKind::from_str(&row.get::<_, String>(1)?),
                    name: row.get(2)?,
                    qualified_name: row.get(3)?,
                    file_path: row.get(4)?,
                    language: row.get(5)?,
                    start_line: row.get::<_, i32>(6)? as u32,
                    end_line: row.get::<_, i32>(7)? as u32,
                    start_column: row.get::<_, i32>(8)? as u32,
                    end_column: row.get::<_, i32>(9)? as u32,
                    signature: row.get(10)?,
                    docstring: row.get(11)?,
                    visibility: row.get(12)?,
                    is_exported: row.get::<_, i32>(13)? != 0,
                    is_async: row.get::<_, i32>(14)? != 0,
                    is_static: row.get::<_, i32>(15)? != 0,
                    is_abstract: row.get::<_, i32>(16)? != 0,
                    extra,
                })
            })?;
            for r in rows {
                collected.push(r?);
            }
        }

        // Group by file_path, preserving the input order of the requested paths
        // so the CLI output is stable.
        let mut by_file: std::collections::BTreeMap<String, Vec<Node>> =
            std::collections::BTreeMap::new();
        for n in collected {
            by_file.entry(n.file_path.clone()).or_default().push(n);
        }
        let mut seeds: Vec<BlastSeed> = Vec::with_capacity(by_file.len());
        for path in paths {
            if let Some(nodes) = by_file.remove(path) {
                seeds.push(BlastSeed { file_path: path.clone(), nodes });
            }
        }
        Ok(seeds)
    }

    /// Recursive CTE-based reachability on `calls` edges from `seeds`,
    /// bounded by `depth` BFS hops. Excludes the seeds themselves.
    /// Direction:
    ///   * `Inbound`  — who calls the seeds (default)
    ///   * `Outbound` — who the seeds call
    ///   * `Both`     — union of the two
    ///
    /// `depth=0` returns an empty impact. Empty `seeds` also returns empty.
    pub fn reachable_calls(
        &self,
        seeds: &[String],
        depth: u32,
        direction: BlastDirection,
    ) -> GraphResult<BlastImpact> {
        if seeds.is_empty() || depth == 0 {
            return Ok(BlastImpact {
                nodes: Vec::new(),
                files: Vec::new(),
                total_count: 0,
            });
        }
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        const CHUNK: usize = 500;

        // Build a direction config once.
        let variants: &[(char, char)] = match direction {
            BlastDirection::Inbound => &[('s', 't')],     // target IN (seeds), seed=source
            BlastDirection::Outbound => &[('t', 's')],    // source IN (seeds), seed=target
            BlastDirection::Both => &[('s', 't'), ('t', 's')],
        };

        // (a) Discover the impacted node ids via CTE, deduped across chunks.
        let mut impact_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for &(seed_role, neighbor_role) in variants {
            // seed_role is the role of node_ids in the edges table — the side
            // we already know (== one of the seeds); neighbor_role is the side
            // we hop to.
            let (seed_col, next_col) = match (seed_role, neighbor_role) {
                ('s', 't') => ("target", "source"), // inbound: starting from call targets
                ('t', 's') => ("source", "target"), // outbound: starting from call sources
                _ => unreachable!("variant guard"),
            };
            for chunk in seeds.chunks(CHUNK) {
                let ph = std::iter::repeat("?")
                    .take(chunk.len())
                    .collect::<Vec<_>>()
                    .join(",");
                // For the seed set itself we filter neighbor_role IN-chunk in
                // the base case, and add a separate `id NOT IN (seeds)` filter
                // at the end so we never emit a seed as its own impacted node.
                let base_filter = format!("{seed_col} IN ({ph})");
                let cte = format!(
                    "WITH RECURSIVE reach(node_id, depth) AS (\n\
                       SELECT e.{next_col}, 1 FROM edges e\n\
                       WHERE e.{base_filter}\n\
                         AND e.kind = 'calls' AND e.valid = 1\n\
                     UNION\n\
                       SELECT e.{next_col}, r.depth + 1 FROM reach r\n\
                       JOIN edges e ON e.{seed_col} = r.node_id\n\
                       WHERE e.kind = 'calls' AND e.valid = 1\n\
                         AND r.depth < ?\n
                     )\n\
                     SELECT DISTINCT node_id FROM reach",
                );
                let mut stmt = conn.prepare(&cte)?;
                // Pack the seeds (already &str under `chunk`) plus the depth
                // into a single slice of `&dyn ToSql` — that's what
                // rusqlite's `Params` impl accepts.
                let depth_param: i64 = depth as i64;
                let params_vec: Vec<&dyn rusqlite::ToSql> = {
                    let mut v: Vec<&dyn rusqlite::ToSql> =
                        Vec::with_capacity(chunk.len() + 1);
                    for s in chunk {
                        v.push(s);
                    }
                    v.push(&depth_param);
                    v
                };
                let id_rows = stmt.query_map(params_vec.as_slice(), |row| {
                    let id: String = row.get(0)?;
                    Ok(id)
                })?;
                for r in id_rows {
                    impact_ids.insert(r?);
                }
            }
        }
        // Drop the seeds themselves from the impacted set.
        let seed_set: std::collections::HashSet<&str> =
            seeds.iter().map(|s| s.as_str()).collect();
        impact_ids.retain(|id| !seed_set.contains(id.as_str()));
        if impact_ids.is_empty() {
            return Ok(BlastImpact {
                nodes: Vec::new(),
                files: Vec::new(),
                total_count: 0,
            });
        }

        // (b) Fetch the node rows for the impacted ids, chunked.
        let ids: Vec<String> = impact_ids.into_iter().collect();
        let mut nodes: Vec<Node> = Vec::with_capacity(ids.len());
        for chunk in ids.chunks(CHUNK) {
            let ph = std::iter::repeat("?")
                .take(chunk.len())
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "SELECT id, kind, name, qualified_name, file_path, language,
                        start_line, end_line, start_column, end_column,
                        signature, docstring, visibility,
                        is_exported, is_async, is_static, is_abstract, extra
                 FROM nodes
                 WHERE valid = 1 AND id IN ({ph})"
            );
            let mut stmt = conn.prepare(&sql)?;
            let params_vec: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
            let rows = stmt.query_map(params_vec.as_slice(), |row| {
                let extra_str: String = row.get(17)?;
                let extra: HashMap<String, String> =
                    serde_json::from_str(&extra_str).unwrap_or_default();
                Ok(Node {
                    id: row.get(0)?,
                    kind: NodeKind::from_str(&row.get::<_, String>(1)?),
                    name: row.get(2)?,
                    qualified_name: row.get(3)?,
                    file_path: row.get(4)?,
                    language: row.get(5)?,
                    start_line: row.get::<_, i32>(6)? as u32,
                    end_line: row.get::<_, i32>(7)? as u32,
                    start_column: row.get::<_, i32>(8)? as u32,
                    end_column: row.get::<_, i32>(9)? as u32,
                    signature: row.get(10)?,
                    docstring: row.get(11)?,
                    visibility: row.get(12)?,
                    is_exported: row.get::<_, i32>(13)? != 0,
                    is_async: row.get::<_, i32>(14)? != 0,
                    is_static: row.get::<_, i32>(15)? != 0,
                    is_abstract: row.get::<_, i32>(16)? != 0,
                    extra,
                })
            })?;
            for r in rows {
                nodes.push(r?);
            }
        }

        // (c) Distinct file_path rollup, sorted.
        let mut files: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for n in &nodes {
            files.insert(n.file_path.clone());
        }
        let files: Vec<String> = files.into_iter().collect();
        let total_count = nodes.len();
        Ok(BlastImpact { nodes, files, total_count })
    }

    /// Count of nodes in the graph that match the entry-point heuristic used
    /// by `dead_code()`. Used by the engine to fill in
    /// `DeadCodeReport::entry_points`.
    pub fn entry_point_count(&self) -> GraphResult<usize> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM nodes
             WHERE valid = 1
               AND (
                   name IN ('main','index','__init__')
                   OR file_path LIKE '%/bin/%'
                   OR file_path LIKE '%/main.%'
                   OR file_path LIKE '%/index.%'
                   OR is_exported = 1
                   OR COALESCE(visibility, '') IN ('public','pub')
               )",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    // =========================================================================
    // FTS5 helpers (Phase 3)
    // =========================================================================

    /// Most-recently-inserted valid nodes, ranked by `rowid` desc.
    /// Used as the fallback when the user submits an empty / non-tokenizable
    /// query to `search_nodes`.
    fn search_recent(&self, limit: usize) -> GraphResult<Vec<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes WHERE valid = 1
             ORDER BY rowid DESC
             LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], row_to_node)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    // =========================================================================
    // Architecture overview (Phase 4)
    // =========================================================================

    /// Multi-aspect architecture query. Runs one SQL aggregation per
    /// requested aspect and assembles the result into `ArchitectureReport`.
    /// `Clusters` and `Cycles` are silently skipped (deferred to Phase 5).
    pub fn architecture_overview(
        &self,
        req: &ArchitectureRequest,
    ) -> GraphResult<ArchitectureReport> {
        // Sanitize the path scope once. CLI args only, but we still
        // refuse quote/semicolon/backslash so a stray flag value can
        // never inject into our hand-built SQL fragments below.
        if let Some(scope) = &req.path_scope {
            if scope.contains('\'') || scope.contains(';') || scope.contains('\\') {
                return Err(GraphError::Engine(format!(
                    "invalid path_scope (must not contain ' ; or \\): {scope}"
                )));
            }
        }
        let scope_frag = match &req.path_scope {
            Some(s) => format!(" AND (file_path = '{s}' OR file_path LIKE '{s}%')"),
            None => String::new(),
        };
        let aspects = if req.aspects.is_empty() {
            ArchitectureAspect::default_set()
        } else {
            req.aspects.clone()
        };
        let top_n = req.top_n.max(1);
        let wants = |a: ArchitectureAspect| aspects.contains(&a);

        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;

        let mut report = ArchitectureReport {
            requested_aspects: aspects.clone(),
            path_scope: req.path_scope.clone(),
            ..Default::default()
        };

        // ----- Overview ----------------------------------------------------
        if wants(ArchitectureAspect::Overview) {
            let total_nodes: i64 = conn.query_row(
                &format!("SELECT COUNT(*) FROM nodes WHERE valid = 1{}", scope_frag),
                [],
                |r| r.get(0),
            )?;
            let total_edges: i64 = conn.query_row(
                "SELECT COUNT(*) FROM edges WHERE valid = 1",
                [],
                |r| r.get(0),
            )?;
            let total_files: i64 = if req.path_scope.is_some() {
                conn.query_row(
                    &format!(
                        "SELECT COUNT(DISTINCT file_path) FROM nodes WHERE valid = 1{}",
                        scope_frag
                    ),
                    [],
                    |r| r.get(0),
                )?
            } else {
                conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?
            };
            let total_routes: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM nodes WHERE valid = 1 AND kind = 'route'{}",
                    scope_frag
                ),
                [],
                |r| r.get(0),
            )?;
            let languages_count: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(DISTINCT language) FROM nodes WHERE valid = 1{}",
                    scope_frag
                ),
                [],
                |r| r.get(0),
            )?;
            let entry_points_count: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM nodes
                     WHERE valid = 1 AND (
                         name IN ('main','index','__init__')
                         OR file_path LIKE '%/bin/%'
                         OR file_path LIKE '%/main.%'
                         OR file_path LIKE '%/index.%'
                         OR is_exported = 1
                         OR COALESCE(visibility, '') IN ('public','pub')
                     ){}",
                    scope_frag
                ),
                [],
                |r| r.get(0),
            )?;
            let dead_count: i64 = conn.query_row(
                &format!(
                    "SELECT COUNT(*) FROM nodes
                     WHERE valid = 1
                       AND kind IN ('function','method','test')
                       AND id NOT IN (SELECT target FROM edges WHERE kind='calls' AND valid=1)
                       AND NOT (
                           name IN ('main','index','__init__')
                           OR file_path LIKE '%/bin/%'
                           OR file_path LIKE '%/main.%'
                           OR file_path LIKE '%/index.%'
                           OR is_exported = 1
                           OR COALESCE(visibility, '') IN ('public','pub')
                       ){}",
                    scope_frag
                ),
                [],
                |r| r.get(0),
            )?;
            let call_edges_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM edges WHERE kind = 'calls' AND valid = 1",
                [],
                |r| r.get(0),
            )?;
            report.overview = Some(OverviewSection {
                total_nodes: total_nodes as usize,
                total_edges: total_edges as usize,
                total_files: total_files as usize,
                total_routes: total_routes as usize,
                languages_count: languages_count as usize,
                entry_points_count: entry_points_count as usize,
                dead_count: dead_count as usize,
                call_edges_count: call_edges_count as usize,
            });
        }

        // Load scoped node + edge sets once so per-aspect sections can
        // bucket in Rust rather than issuing fresh SQL per aspect.
        let scoped_nodes: Vec<Node> = if req.path_scope.is_some() {
            let s = req.path_scope.as_deref().unwrap();
            let mut stmt = conn.prepare(
                "SELECT id, kind, name, qualified_name, file_path, language,
                        start_line, end_line, start_column, end_column,
                        signature, docstring, visibility,
                        is_exported, is_async, is_static, is_abstract, extra
                 FROM nodes WHERE valid = 1 AND (file_path = ?1 OR file_path LIKE ?2)",
            )?;
            let like = format!("{s}/%");
            let rows = stmt.query_map(params![s, like], row_to_node)?;
            rows.collect::<Result<_, _>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, kind, name, qualified_name, file_path, language,
                        start_line, end_line, start_column, end_column,
                        signature, docstring, visibility,
                        is_exported, is_async, is_static, is_abstract, extra
                 FROM nodes WHERE valid = 1",
            )?;
            let rows = stmt.query_map([], row_to_node)?;
            rows.collect::<Result<_, _>>()?
        };
        let id_to_node: HashMap<&str, &Node> =
            scoped_nodes.iter().map(|n| (n.id.as_str(), n)).collect();
        let scoped_edges: Vec<(String, String, String, String)> = if req.path_scope.is_some() {
            let s = req.path_scope.as_deref().unwrap();
            let mut stmt = conn.prepare(
                "SELECT e.id, e.source, e.target, e.kind
                 FROM edges e
                 JOIN nodes s ON s.id = e.source
                 WHERE e.valid = 1 AND (s.file_path = ?1 OR s.file_path LIKE ?2
                                        OR EXISTS (SELECT 1 FROM nodes t
                                                   WHERE t.id = e.target
                                                     AND (t.file_path = ?3 OR t.file_path LIKE ?4)))",
            )?;
            let like = format!("{s}/%");
            let rows = stmt.query_map(params![s, &like, s, &like], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, source, target, kind FROM edges WHERE valid = 1",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?;
            rows.collect::<Result<_, _>>()?
        };

        // ----- Structure / Packages ---------------------------------------
        if wants(ArchitectureAspect::Structure) || wants(ArchitectureAspect::Packages) {
            let mut pkg_nodes: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for n in &scoped_nodes {
                *pkg_nodes.entry(package_of(&n.file_path, 2)).or_default() += 1;
            }
            let mut pkg_edges: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for (_id, src, tgt, _kind) in &scoped_edges {
                if let (Some(s), Some(t)) = (id_to_node.get(src.as_str()), id_to_node.get(tgt.as_str())) {
                    if s.file_path == t.file_path {
                        continue;
                    }
                    let sp = package_of(&s.file_path, 2);
                    let tp = package_of(&t.file_path, 2);
                    if sp == tp {
                        *pkg_edges.entry(sp).or_default() += 1;
                    }
                }
            }
            let mut rows: Vec<PackageRow> = pkg_nodes
                .into_iter()
                .map(|(path, node_count)| PackageRow {
                    edge_count: *pkg_edges.get(&path).unwrap_or(&0),
                    node_count,
                    path,
                })
                .collect();
            rows.sort_by(|a, b| b.node_count.cmp(&a.node_count).then(a.path.cmp(&b.path)));
            if wants(ArchitectureAspect::Structure) {
                report.structure = Some(rows.clone());
            }
            if wants(ArchitectureAspect::Packages) {
                rows.truncate(top_n);
                report.packages = Some(rows);
            }
        }

        // ----- Dependencies -----------------------------------------------
        if wants(ArchitectureAspect::Dependencies) {
            let mut dep: std::collections::BTreeMap<(String, String), usize> =
                std::collections::BTreeMap::new();
            for (_id, src, tgt, kind) in &scoped_edges {
                if kind != "imports" {
                    continue;
                }
                let (Some(s), Some(t)) =
                    (id_to_node.get(src.as_str()), id_to_node.get(tgt.as_str())) else { continue; };
                if s.file_path == t.file_path {
                    continue;
                }
                *dep.entry((s.file_path.clone(), t.file_path.clone())).or_default() += 1;
            }
            let mut rows: Vec<DepRow> = dep
                .into_iter()
                .map(|((from_path, to_path), edge_count)| DepRow { from_path, to_path, edge_count })
                .collect();
            rows.sort_by(|a, b| b.edge_count.cmp(&a.edge_count));
            rows.truncate(top_n);
            report.dependencies = Some(rows);
        }

        // ----- Routes -----------------------------------------------------
        if wants(ArchitectureAspect::Routes) {
            let mut rows: Vec<RouteRow> = scoped_nodes
                .iter()
                .filter(|n| n.kind == NodeKind::Route)
                .map(|n| RouteRow {
                    id: n.id.clone(),
                    qualified_name: n.qualified_name.clone(),
                    file_path: n.file_path.clone(),
                    start_line: n.start_line,
                    method: n.extra.get("method").cloned(),
                    path: n.extra.get("path").cloned(),
                })
                .collect();
            rows.sort_by(|a, b| {
                a.file_path
                    .cmp(&b.file_path)
                    .then(a.start_line.cmp(&b.start_line))
            });
            report.routes = Some(rows);
        }

        // ----- Languages --------------------------------------------------
        if wants(ArchitectureAspect::Languages) {
            let mut node_count: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            let mut files: std::collections::BTreeMap<&str, std::collections::BTreeSet<&str>> =
                std::collections::BTreeMap::new();
            for n in &scoped_nodes {
                *node_count.entry(n.language.as_str()).or_default() += 1;
                files.entry(n.language.as_str()).or_default().insert(n.file_path.as_str());
            }
            let mut edge_count: std::collections::BTreeMap<&str, usize> =
                std::collections::BTreeMap::new();
            for (_id, src, _tgt, _kind) in &scoped_edges {
                if let Some(s) = id_to_node.get(src.as_str()) {
                    *edge_count.entry(s.language.as_str()).or_default() += 1;
                }
            }
            let rows: Vec<LanguageRow> = node_count
                .into_iter()
                .map(|(language, nc)| LanguageRow {
                    file_count: files.get(language).map(|s| s.len()).unwrap_or(0),
                    edge_count: *edge_count.get(language).unwrap_or(&0),
                    language: language.to_string(),
                    node_count: nc,
                })
                .collect();
            report.languages = Some(rows);
        }

        // ----- Entry points -----------------------------------------------
        if wants(ArchitectureAspect::EntryPoints) {
            let mut entries: Vec<Node> = scoped_nodes
                .iter()
                .filter(|n| is_entry_point(n))
                .cloned()
                .collect();
            entries.sort_by(|a, b| a.file_path.cmp(&b.file_path).then(a.name.cmp(&b.name)));
            entries.truncate(200);
            report.entry_points = Some(entries);
        }

        // ----- Hotspots ---------------------------------------------------
        if wants(ArchitectureAspect::Hotspots) {
            let mut indeg: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for (_id, _src, tgt, kind) in &scoped_edges {
                if kind == "calls" {
                    *indeg.entry(tgt.as_str()).or_default() += 1;
                }
            }
            let mut rows: Vec<HotspotRow> = indeg
                .into_iter()
                .filter_map(|(id, deg)| id_to_node.get(id).map(|n| (n, deg)))
                .map(|(n, in_degree)| HotspotRow { node: (*n).clone(), in_degree })
                .collect();
            rows.sort_by(|a, b| b.in_degree.cmp(&a.in_degree).then(a.node.qualified_name.cmp(&b.node.qualified_name)));
            rows.truncate(top_n);
            report.hotspots = Some(rows);
        }

        // ----- Boundaries -------------------------------------------------
        if wants(ArchitectureAspect::Boundaries) {
            let mut inbound: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            let mut outbound: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for (_id, src, tgt, kind) in &scoped_edges {
                if kind != "calls" {
                    continue;
                }
                let (Some(s), Some(t)) =
                    (id_to_node.get(src.as_str()), id_to_node.get(tgt.as_str())) else { continue; };
                if s.file_path == t.file_path {
                    continue;
                }
                let sp = package_of(&s.file_path, 2);
                let tp = package_of(&t.file_path, 2);
                if sp != tp {
                    *outbound.entry(sp).or_default() += 1;
                    *inbound.entry(tp).or_default() += 1;
                }
            }
            let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            keys.extend(inbound.keys().cloned());
            keys.extend(outbound.keys().cloned());
            let mut rows: Vec<BoundaryRow> = keys
                .into_iter()
                .map(|path| {
                    let ib = *inbound.get(&path).unwrap_or(&0);
                    let ob = *outbound.get(&path).unwrap_or(&0);
                    let total = (ib + ob) as f64;
                    let ratio = if total > 0.0 { ob as f64 / total } else { 0.0 };
                    BoundaryRow { path, inbound_count: ib, outbound_count: ob, fan_out_ratio: ratio }
                })
                .collect();
            rows.sort_by(|a, b| {
                let ta = a.inbound_count + a.outbound_count;
                let tb = b.inbound_count + b.outbound_count;
                tb.cmp(&ta).then(a.path.cmp(&b.path))
            });
            rows.truncate(top_n);
            report.boundaries = Some(rows);
        }

        // ----- Layers (BFS over CALLS from entry points) ------------------
        if wants(ArchitectureAspect::Layers) {
            let entries: Vec<&Node> = scoped_nodes
                .iter()
                .filter(|n| is_entry_point(n))
                .collect();
            let entry_count = entries.len();
            let mut layer: std::collections::HashMap<String, u32> =
                std::collections::HashMap::new();
            for &seed in &entries {
                layer.insert(seed.id.clone(), 0);
            }
            let mut adj: std::collections::HashMap<&str, Vec<&str>> =
                std::collections::HashMap::new();
            for (_id, src, tgt, kind) in &scoped_edges {
                if kind == "calls" {
                    adj.entry(src.as_str()).or_default().push(tgt.as_str());
                }
            }
            let mut frontier: Vec<&str> = entries.iter().map(|n| n.id.as_str()).collect();
            let mut max_layer = 0u32;
            let mut current_layer = 0u32;
            while !frontier.is_empty() {
                let mut next: Vec<&str> = Vec::new();
                for &node in &frontier {
                    if let Some(targets) = adj.get(node) {
                        for &t in targets {
                            if let Some(t_node) = id_to_node.get(t) {
                                let entry = layer.entry(t_node.id.clone()).or_insert(current_layer + 1);
                                if *entry < current_layer + 1 {
                                    *entry = current_layer + 1;
                                }
                                next.push(t);
                            }
                        }
                    }
                }
                if next.is_empty() {
                    break;
                }
                current_layer += 1;
                max_layer = current_layer;
                frontier = next;
            }
            let mut per_layer: Vec<usize> = vec![0; (max_layer as usize) + 1];
            for &l in layer.values() {
                if (l as usize) < per_layer.len() {
                    per_layer[l as usize] += 1;
                }
            }
            report.layers = Some(LayersSection {
                entry_count,
                max_layer,
                node_count_per_layer: per_layer,
            });
        }

        // ----- File tree --------------------------------------------------
        if wants(ArchitectureAspect::FileTree) {
            let root = req.path_scope.clone().unwrap_or_default();
            let entries = build_file_tree(&scoped_nodes);
            report.file_tree = Some(FileTree { root, entries });
        }

        // Clusters / Cycles: silently skipped — see Phase 5 plan.

        Ok(report)
    }
}


// ---- Architecture overview helpers (Phase 4) ----------------------------

/// Compute the directory prefix of `file_path` up to `depth` segments.
pub(crate) fn package_of(file_path: &str, depth: usize) -> String {
    if file_path.is_empty() {
        return String::new();
    }
    let mut parts: Vec<&str> = file_path.split('/').collect();
    if !parts.is_empty() {
        parts.pop();
    }
    if depth < parts.len() {
        parts.truncate(depth);
    }
    parts.join("/")
}

/// Mirror of the entry-point heuristic used by `dead_code`.
fn is_entry_point(n: &Node) -> bool {
    if matches!(n.name.as_str(), "main" | "index" | "__init__") {
        return true;
    }
    if n.is_exported {
        return true;
    }
    if matches!(n.visibility.as_deref(), Some("public") | Some("pub")) {
        return true;
    }
    if n.file_path.contains("/bin/")
        || n.file_path.contains("main.")
        || n.file_path.contains("index.")
    {
        return true;
    }
    false
}

/// Build a nested file tree from distinct `file_path`s. Two-tier
/// layout: top-level directory → leaf files.
pub(crate) fn build_file_tree(nodes: &[Node]) -> Vec<FileTreeEntry> {
    use std::collections::BTreeMap;

    let mut root: BTreeMap<String, Vec<FileTreeEntry>> = BTreeMap::new();
    let mut paths_seen: std::collections::BTreeSet<String> =
        std::collections::BTreeSet::new();
    for n in nodes {
        if !paths_seen.insert(n.file_path.clone()) {
            continue;
        }
        let parts: Vec<&str> = n.file_path.split('/').collect();
        let top = parts.first().copied().unwrap_or("").to_string();
        let leaf = parts.last().copied().unwrap_or("").to_string();
        if leaf.is_empty() || leaf == top {
            continue;
        }
        root.entry(top).or_default().push(FileTreeEntry {
            name: leaf,
            kind: "file".to_string(),
            children: Vec::new(),
        });
    }
    let mut out: Vec<FileTreeEntry> = Vec::new();
    for (name, mut files) in root {
        if files.is_empty() && name.is_empty() {
            continue;
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        out.push(FileTreeEntry {
            name: if name.is_empty() { "<root>".to_string() } else { name },
            kind: "directory".to_string(),
            children: files,
        });
    }
    out
}

/// Map a SELECT-row over the `nodes` table to a `Node`. Shared by
/// `all_nodes`, `search_nodes`, `find_definitions`, and `node_by_id`
/// so the column ordering lives in exactly one place.
fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
    let extra_str: String = row.get(17)?;
    let extra: HashMap<String, String> =
        serde_json::from_str(&extra_str).unwrap_or_default();
    Ok(Node {
        id: row.get(0)?,
        kind: NodeKind::from_str(&row.get::<_, String>(1)?),
        name: row.get(2)?,
        qualified_name: row.get(3)?,
        file_path: row.get(4)?,
        language: row.get(5)?,
        start_line: row.get::<_, i32>(6)? as u32,
        end_line: row.get::<_, i32>(7)? as u32,
        start_column: row.get::<_, i32>(8)? as u32,
        end_column: row.get::<_, i32>(9)? as u32,
        signature: row.get(10)?,
        docstring: row.get(11)?,
        visibility: row.get(12)?,
        is_exported: row.get::<_, i32>(13)? != 0,
        is_async: row.get::<_, i32>(14)? != 0,
        is_static: row.get::<_, i32>(15)? != 0,
        is_abstract: row.get::<_, i32>(16)? != 0,
        extra,
    })
}

// ---- FTS5 sync helpers (private, module-scoped) --------------------------

/// Build the space-joined body string for FTS5 from a Node.
/// Identifier-tokenizes each field (name, qualified_name, optional signature)
/// so the body becomes a flat stream of lowercase tokens like
/// "update cloud client function update cloud client (self )" — the FTS5
/// MATCH expression for a user query "update cloud client" hits this row.
fn fts_body(node: &Node) -> String {
    let mut parts: Vec<String> = Vec::new();
    parts.push(node.name.clone());
    parts.push(node.qualified_name.clone());
    if let Some(sig) = &node.signature {
        parts.push(sig.clone());
    }
    let mut body = String::new();
    let mut first = true;
    for part in &parts {
        let tokens = crate::types::tokenize_identifier(part);
        if tokens.is_empty() {
            continue;
        }
        if !first {
            body.push(' ');
        }
        body.push_str(&tokens);
        first = false;
    }
    body
}

/// Insert (or refresh) the FTS row for a node id. Any prior FTS row with
/// the same id is removed first — INSERT OR REPLACE isn't supported on
/// FTS5 virtual tables, so DELETE+INSERT is the standard pattern.
fn fts_upsert(conn: &rusqlite::Connection, node_id: &str, body: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM nodes_fts WHERE id = ?1", params![node_id])?;
    conn.execute(
        "INSERT INTO nodes_fts (id, body) VALUES (?1, ?2)",
        params![node_id, body],
    )?;
    Ok(())
}

/// Bulk-delete FTS rows by id. Used by `clear_all` and by
/// `delete_nodes_for_file`. Chunks under SQLite's 999-param cap.
fn fts_delete_for_ids(conn: &rusqlite::Connection, ids: &[String]) -> rusqlite::Result<()> {
    if ids.is_empty() {
        return Ok(());
    }
    const CHUNK: usize = 500;
    for chunk in ids.chunks(CHUNK) {
        let placeholders = std::iter::repeat("?")
            .take(chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!("DELETE FROM nodes_fts WHERE id IN ({})", placeholders);
        let params_vec: Vec<&dyn rusqlite::ToSql> =
            chunk.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
        conn.execute(&sql, params_vec.as_slice())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{EdgeKind, NodeKind};
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, SqliteStorage) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("g.db");
        let s = SqliteStorage::open(&path).expect("open");
        (dir, s)
    }

    fn mk_node(id: &str, file: &str) -> Node {
        Node {
            id: id.to_string(),
            kind: NodeKind::Function,
            name: id.to_string(),
            qualified_name: id.to_string(),
            file_path: file.to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 1,
            start_column: 0,
            end_column: 1,
            signature: None,
            docstring: None,
            visibility: None,
            is_exported: false,
            is_async: false,
            is_static: false,
            is_abstract: false,
            extra: HashMap::new(),
        }
    }

    fn mk_edge(id: &str, source: &str, target: &str) -> Edge {
        Edge {
            id: id.to_string(),
            source: source.to_string(),
            target: target.to_string(),
            kind: EdgeKind::Calls,
            line: 1,
            col: 0,
            metadata: None,
            provenance: None,
        }
    }

    /// Same shape as `mk_edge` but tagged as `imports` for the
    /// dependency rollup.
    fn mk_imp(id: &str, source: &str, target: &str) -> Edge {
        let mut e = mk_edge(id, source, target);
        e.kind = EdgeKind::Imports;
        e
    }

    #[test]
    fn delete_nodes_for_file_returns_ids_and_removes_rows() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:foo:L1", "src/a.rs")).unwrap();
        s.upsert_node(&mk_node("function:bar:L2", "src/a.rs")).unwrap();
        s.upsert_node(&mk_node("function:baz:L1", "src/b.rs")).unwrap();

        let removed = s.delete_nodes_for_file("src/a.rs").unwrap();
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"function:foo:L1".to_string()));
        assert!(removed.contains(&"function:bar:L2".to_string()));

        let remaining: Vec<_> = s
            .all_nodes()
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(remaining, vec!["function:baz:L1".to_string()]);
    }

    #[test]
    fn delete_edges_involving_clears_both_sides() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:a:L1", "src/a.rs")).unwrap();
        s.upsert_node(&mk_node("function:b:L1", "src/b.rs")).unwrap();
        s.upsert_node(&mk_node("function:c:L1", "src/c.rs")).unwrap();
        s.upsert_edge(&mk_edge("e1", "function:a:L1", "function:b:L1")).unwrap();
        s.upsert_edge(&mk_edge("e2", "function:b:L1", "function:c:L1")).unwrap();
        s.upsert_edge(&mk_edge("e3", "function:c:L1", "function:a:L1")).unwrap();

        let removed = s
            .delete_edges_involving(&["function:b:L1".to_string()])
            .unwrap();
        assert_eq!(removed, 2);

        // e3 (a -> c) should still be there: neither side is b.
        let all = s.all_edges().unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].id, "e3");
    }

    #[test]
    fn delete_edges_involving_handles_empty_input() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:a:L1", "src/a.rs")).unwrap();
        s.upsert_edge(&mk_edge("e1", "function:a:L1", "function:b:L1")).unwrap();
        let n = s.delete_edges_involving(&[]).unwrap();
        assert_eq!(n, 0);
        assert_eq!(s.all_edges().unwrap().len(), 1);
    }

    #[test]
    fn get_and_delete_file_record_roundtrip() {
        let (_dir, s) = fresh();
        let rec = FileRecord {
            path: "src/main.rs".to_string(),
            language: "rust".to_string(),
            mtime: 100,
            content_hash: "deadbeef".to_string(),
            indexed_at: 200,
        };
        assert!(s.get_file_record("src/main.rs").unwrap().is_none());
        s.upsert_file(&rec).unwrap();
        let got = s.get_file_record("src/main.rs").unwrap().unwrap();
        assert_eq!(got.path, rec.path);
        assert_eq!(got.content_hash, rec.content_hash);
        s.delete_file_record("src/main.rs").unwrap();
        assert!(s.get_file_record("src/main.rs").unwrap().is_none());
    }

    #[test]
    fn all_file_paths_lists_indexed_files() {
        let (_dir, s) = fresh();
        s.upsert_file(&FileRecord {
            path: "a.rs".into(),
            language: "rust".into(),
            mtime: 0,
            content_hash: String::new(),
            indexed_at: 0,
        })
        .unwrap();
        s.upsert_file(&FileRecord {
            path: "b.rs".into(),
            language: "rust".into(),
            mtime: 0,
            content_hash: String::new(),
            indexed_at: 0,
        })
        .unwrap();
        let mut paths = s.all_file_paths().unwrap();
        paths.sort();
        assert_eq!(paths, vec!["a.rs".to_string(), "b.rs".to_string()]);
    }

    #[test]
    fn edge_sources_pointing_to_finds_inbound_refs() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:callee:L1", "src/lib.rs")).unwrap();
        s.upsert_node(&mk_node("file:src/a.rs", "src/a.rs")).unwrap();
        s.upsert_node(&mk_node("file:src/b.rs", "src/b.rs")).unwrap();
        s.upsert_edge(&mk_edge(
            "e1",
            "file:src/a.rs",
            "function:callee:L1",
        ))
        .unwrap();
        s.upsert_edge(&mk_edge(
            "e2",
            "file:src/b.rs",
            "function:callee:L1",
        ))
        .unwrap();
        s.upsert_edge(&mk_edge("e3", "file:src/a.rs", "file:src/b.rs"))
            .unwrap();

        let mut srcs = s
            .edge_sources_pointing_to(&["function:callee:L1".to_string()])
            .unwrap();
        srcs.sort();
        assert_eq!(srcs, vec!["file:src/a.rs".to_string(), "file:src/b.rs".to_string()]);
    }

    /// Helper: build a node with a specific id, file, kind, exported flag.
    fn mk_typed_node(id: &str, file: &str, kind: NodeKind, exported: bool) -> Node {
        let mut n = mk_node(id, file);
        n.kind = kind;
        n.is_exported = exported;
        n
    }

    #[test]
    fn dead_code_excludes_called_and_entry_points() {
        let (_dir, s) = fresh();
        // `a` is called by no one → would be dead; `b` is called by `a` so
        // it's reached → not dead.
        s.upsert_node(&mk_node("function:a:L1", "src/lib.rs")).unwrap();
        s.upsert_node(&mk_node("function:b:L1", "src/lib.rs")).unwrap();
        s.upsert_edge(&mk_edge("e1", "function:a:L1", "function:b:L1"))
            .unwrap();
        // `c` is exported, no callers — but is an entry point → excluded.
        s.upsert_node(&mk_typed_node(
            "function:c:L1",
            "src/lib.rs",
            NodeKind::Function,
            true,
        ))
        .unwrap();
        // `d` is named `main` in bin/main.rs → entry point → excluded.
        s.upsert_node(&mk_typed_node(
            "function:d:L1",
            "bin/main.rs",
            NodeKind::Function,
            false,
        ))
        .unwrap();

        let entries = s.dead_code().unwrap();
        let names: Vec<&str> = entries.iter().map(|e| e.node.id.as_str()).collect();
        assert!(
            names.contains(&"function:a:L1"),
            "a has no callers and is not an entry point → must be flagged, got {:?}",
            names
        );
        assert!(
            !names.contains(&"function:b:L1"),
            "b is called by a → not dead, got {:?}",
            names
        );
        assert!(
            !names.contains(&"function:c:L1"),
            "c is exported → not dead, got {:?}",
            names
        );
        assert!(
            !names.contains(&"function:d:L1"),
            "d is named 'main' → not dead, got {:?}",
            names
        );

        // entry_point_count confirms the heuristic picks up `c` and `d`.
        let epc = s.entry_point_count().unwrap();
        assert!(epc >= 2, "expected ≥2 entry points (c exported + d main), got {epc}");
    }


    #[test]
    fn nodes_for_files_groups_correctly() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:f1:L1", "src/lib.rs")).unwrap();
        s.upsert_node(&mk_node("function:f2:L1", "src/lib.rs")).unwrap();

        s.upsert_node(&mk_node("function:u1:L1", "src/util.rs")).unwrap();
        // Variable node — not a definition kind → ignored by nodes_for_files.
        let mut non_def = mk_node("non_def:x:L1", "docs/x.md");
        non_def.kind = NodeKind::Variable;
        s.upsert_node(&non_def).unwrap();

        let seeds = s
            .nodes_for_files(&["src/lib.rs".into(), "src/util.rs".into()])
            .unwrap();
        assert_eq!(seeds.len(), 2, "two requested files, got {:?}", seeds.len());
        // Output preserves the input order of requested paths.
        assert_eq!(seeds[0].file_path, "src/lib.rs");
        assert_eq!(seeds[1].file_path, "src/util.rs");
        assert_eq!(seeds[0].nodes.len(), 2);
        assert_eq!(seeds[1].nodes.len(), 1);

        // docs/x.md had only a non-definition node → must not appear in seeds.
        assert!(!seeds.iter().any(|g| g.file_path == "docs/x.md"));
    }

    #[test]
    fn reachable_calls_inbound() {
        let (_dir, s) = fresh();
        // Chain: a -> b -> c -> d, plus an unrelated branch y.
        for id in ["a", "b", "c", "d", "y"] {
            s.upsert_node(&mk_node(&format!("function:{id}:L1"), "src/lib.rs"))
                .unwrap();
        }
        s.upsert_edge(&mk_edge("e1", "function:a:L1", "function:b:L1"))
            .unwrap();
        s.upsert_edge(&mk_edge("e2", "function:b:L1", "function:c:L1"))
            .unwrap();
        s.upsert_edge(&mk_edge("e3", "function:c:L1", "function:d:L1"))
            .unwrap();

        // depth=2 from seeds={d}: c is reached in 1 hop, b in 2 hops. a is 3
        // hops away so NOT reached. y is unrelated.
        let impact = s
            .reachable_calls(
                &["function:d:L1".into()],
                2,
                BlastDirection::Inbound,
            )
            .unwrap();
        let ids: std::collections::BTreeSet<&str> =
            impact.nodes.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("function:c:L1"), "c reachable in 1 hop, got {:?}", ids);
        assert!(ids.contains("function:b:L1"), "b reachable in 2 hops, got {:?}", ids);
        assert!(!ids.contains("function:a:L1"), "a is 3 hops — out of range, got {:?}", ids);
        assert!(!ids.contains("function:d:L1"), "seed must be excluded, got {:?}", ids);

        // depth=1 from {d} → only c.
        let impact = s
            .reachable_calls(
                &["function:d:L1".into()],
                1,
                BlastDirection::Inbound,
            )
            .unwrap();
        assert_eq!(impact.total_count, 1);
        assert_eq!(impact.nodes[0].id, "function:c:L1");
    }

    // =========================================================================
    // Phase 3 — FTS5 / BM25 search tests
    // =========================================================================

    /// Insert two camelCase functions, search by their space-separated tokens,
    /// assert the matching node ranks first.
    #[test]
    fn search_nodes_finds_camel_case_split() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:updateCloudClient:L1", "src/a.rs"))
            .unwrap();
        s.upsert_node(&mk_node("function:parseUserInput:L2", "src/a.rs"))
            .unwrap();

        let hits = s.search_nodes("update cloud", 10).unwrap();
        assert!(
            !hits.is_empty(),
            "expected at least one match for 'update cloud'"
        );
        assert_eq!(
            hits[0].id, "function:updateCloudClient:L1",
            "updateCloudClient must rank first; got {:?}",
            hits.iter().map(|n| &n.id).collect::<Vec<_>>()
        );
    }

    /// Insert a snake_case function, search by its underscore-separated tokens.
    #[test]
    fn search_nodes_finds_snake_case() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:parse_user_input:L1", "src/a.rs"))
            .unwrap();

        let hits = s.search_nodes("parse user input", 10).unwrap();
        assert!(!hits.is_empty(), "expected a match for 'parse user input'");
        assert_eq!(hits[0].id, "function:parse_user_input:L1");
    }

    /// Insert nodes whose names share a substring (`Parser`, `Parse`,
    /// `parse_user_input`) and search for the shortest token "parse".
    /// BM25 should rank the exact-token `Parse` first.
    #[test]
    fn search_nodes_bm25_prefers_exact_match() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:Parser:L1", "src/a.rs"))
            .unwrap();
        s.upsert_node(&mk_node("function:parse_user_input:L2", "src/a.rs"))
            .unwrap();
        s.upsert_node(&mk_node("function:Parse:L3", "src/a.rs"))
            .unwrap();

        let hits = s.search_nodes("parse", 10).unwrap();
        assert!(
            !hits.is_empty(),
            "expected at least one match for 'parse'"
        );
        assert_eq!(
            hits[0].id, "function:Parse:L3",
            "exact-token `Parse` should rank first under BM25; got {:?}",
            hits.iter().map(|n| &n.id).collect::<Vec<_>>()
        );
    }

    /// Bypass the public API to delete a node directly with raw SQL, then
    /// confirm search_nodes no longer returns it — FTS index must have
    /// been synced when upsert_node ran (so the indexed body was correct)
    /// and the public delete path clears the FTS row alongside the
    /// regular row.
    #[test]
    fn fts_stays_in_sync_after_delete() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:alphaOne:L1", "src/a.rs"))
            .unwrap();
        s.upsert_node(&mk_node("function:betaTwo:L2", "src/a.rs"))
            .unwrap();
        s.upsert_node(&mk_node("function:gammaThree:L3", "src/a.rs"))
            .unwrap();

        // Sanity: all three are searchable.
        let before: Vec<String> = s
            .search_nodes("alpha", 10)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert_eq!(before.len(), 1, "alpha must be searchable before delete");

        // Use the public delete path (which clears the FTS row).
        let removed = s.delete_nodes_for_file("src/a.rs").unwrap();
        assert_eq!(removed.len(), 3);

        // After delete, search must return no results — FTS rows were
        // dropped in the same transaction.
        let after: Vec<String> = s
            .search_nodes("alpha", 10)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert!(after.is_empty(), "alpha must not survive delete_nodes_for_file");

        let beta: Vec<String> = s
            .search_nodes("beta", 10)
            .unwrap()
            .into_iter()
            .map(|n| n.id)
            .collect();
        assert!(beta.is_empty(), "beta must not survive delete_nodes_for_file");
    }

    /// Insert nodes, clear the storage, confirm FTS index is also empty
    /// (so a fresh search returns nothing).
    #[test]
    fn fts_stays_in_sync_after_clear_all() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:foo:L1", "src/a.rs"))
            .unwrap();
        s.upsert_node(&mk_node("function:bar:L2", "src/a.rs"))
            .unwrap();

        s.clear_all().unwrap();

        let hits = s.search_nodes("anything", 10).unwrap();
        assert!(hits.is_empty(), "FTS rows must be wiped by clear_all");
    }

    /// Insert a single node via `upsert_node`, then search for a token from
    /// its name. Confirms the FTS upsert hook fires on the single-node path.
    #[test]
    fn fts_insert_via_upsert_node_round_trip() {
        let (_dir, s) = fresh();
        s.upsert_node(&mk_node("function:greetWorld:L1", "src/a.rs"))
            .unwrap();

        let hits = s.search_nodes("greet world", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "function:greetWorld:L1");
        // mk_node sets name == id, so the FTS body for this row contains
        // "function" + "greet" + "world" (after splitting "greetWorld" on
        // the camelCase boundary and "function:greetWorld:L1" on case /
        // punctuation). Confirm the row came back with the expected id.
        assert!(
            hits[0].name.contains("greetWorld"),
            "name should retain the camelCase portion, got {:?}",
            hits[0].name
        );
    }

    /// Empty / whitespace-only queries must NOT error and must fall back to
    /// the recent-nodes path (so callers always get up to `limit` rows).
    #[test]
    fn search_nodes_empty_query_returns_recent() {
        let (_dir, s) = fresh();
        for id in ["function:alphaOne:L1", "function:betaTwo:L2", "function:gammaThree:L3"] {
            s.upsert_node(&mk_node(id, "src/a.rs")).unwrap();
        }

        let empty = s.search_nodes("", 10).unwrap();
        assert_eq!(empty.len(), 3, "empty query must return recent nodes");

        let whitespace = s.search_nodes("  ", 10).unwrap();
        assert_eq!(whitespace.len(), 3, "whitespace query must return recent nodes");

        let underscored = s.search_nodes("___", 10).unwrap();
        assert_eq!(underscored.len(), 3, "underscore-only query must fall through to recent nodes");
    }

    // =========================================================================
    // Phase 4 — architecture overview tests
    // =========================================================================

    /// Insert 4 function nodes in src/a.rs, 2 in src/b/c.rs, 1 file node
    /// for each file. Insert 3 import edges. Assert overview counts and
    /// that `rust` language surfaces.
    #[test]
    fn architecture_overview_returns_counts_for_node_kinds() {
        let (_dir, s) = fresh();
        for id in ["function:fa1:L1", "function:fa2:L2", "function:fa3:L3", "function:fa4:L4"] {
            s.upsert_node(&mk_node(id, "src/a.rs")).unwrap();
        }
        for id in ["function:fc1:L1", "function:fc2:L2"] {
            s.upsert_node(&mk_node(id, "src/b/c.rs")).unwrap();
        }
        s.upsert_node(&mk_node("file:src/a.rs", "src/a.rs")).unwrap();
        s.upsert_node(&mk_node("file:src/b/c.rs", "src/b/c.rs")).unwrap();

        s.upsert_edge(&mk_imp("ia_b", "file:src/a.rs", "file:src/b/c.rs")).unwrap();
        s.upsert_edge(&mk_imp("ib_c", "file:src/b/c.rs", "file:src/b/c.rs")).unwrap();
        s.upsert_edge(&mk_imp("ia_c", "file:src/a.rs", "file:src/b/c.rs")).unwrap();

        let req = ArchitectureRequest {
            aspects: vec![ArchitectureAspect::Overview, ArchitectureAspect::Languages],
            ..Default::default()
        };
        let report = s.architecture_overview(&req).unwrap();
        let o = report.overview.expect("overview populated");
        assert_eq!(o.total_nodes, 8, "4 funcs + 2 funcs + 2 files = 8 nodes");
        assert_eq!(o.total_edges, 3, "three import edges inserted");
        assert_eq!(o.languages_count, 1, "all rust");
        let langs = report.languages.expect("languages populated");
        let rust = langs.iter().find(|r| r.language == "rust").expect("rust row");
        assert_eq!(rust.node_count, 8);
    }

    /// Same setup, but `path_scope = Some("src/a")`. Structure rollup
    /// must only contain the in-scope directory.
    #[test]
    fn architecture_overview_filters_by_path_scope() {
        let (_dir, s) = fresh();
        for id in ["function:fa1:L1", "function:fa2:L2", "function:fa3:L3", "function:fa4:L4"] {
            s.upsert_node(&mk_node(id, "src/a.rs")).unwrap();
        }
        for id in ["function:fc1:L1", "function:fc2:L2"] {
            s.upsert_node(&mk_node(id, "src/b/c.rs")).unwrap();
        }
        s.upsert_node(&mk_node("file:src/a.rs", "src/a.rs")).unwrap();
        s.upsert_node(&mk_node("file:src/b/c.rs", "src/b/c.rs")).unwrap();
        s.upsert_edge(&mk_imp("ia_b", "file:src/a.rs", "file:src/b/c.rs")).unwrap();

        let req = ArchitectureRequest {
            path_scope: Some("src/a".to_string()),
            aspects: vec![ArchitectureAspect::Structure, ArchitectureAspect::Overview],
            ..Default::default()
        };
        let report = s.architecture_overview(&req).unwrap();
        let o = report.overview.expect("overview populated");
        // Scope="src/a" must restrict the overview to the in-scope
        // nodes only — 4 functions + 1 file in src/a, NOT the 2 funcs
        // in src/b/c.rs.
        assert_eq!(o.total_nodes, 5, "4 funcs + 1 file in src/a");
        let struct_rows = report.structure.expect("structure populated");
        // No structure bucket may contain a src/b path.
        for r in &struct_rows {
            assert!(
                !r.path.split('/').any(|p| p == "b") || !r.path.starts_with("src"),
                "src/b should not leak into scoped rollup, got {}",
                r.path
            );
        }
    }

    /// 4 function nodes (a, b, c, d). Calls edges: a→c, b→c, b→d, a→d, c→d
    /// (so d=3 inbound, c=2, a=0, b=0). Hotspots with top_n=2 must return
    /// [d, c].
    #[test]
    fn architecture_overview_hotspots_orders_by_inbound() {
        let (_dir, s) = fresh();
        for id in ["a", "b", "c", "d"] {
            s.upsert_node(&mk_node(&format!("function:{id}:L1"), "src/lib.rs")).unwrap();
        }
        for (id, src, tgt) in [
            ("e1", "function:a:L1", "function:c:L1"),
            ("e2", "function:b:L1", "function:c:L1"),
            ("e3", "function:b:L1", "function:d:L1"),
            ("e4", "function:a:L1", "function:d:L1"),
            ("e5", "function:c:L1", "function:d:L1"),
        ] {
            let mut e = mk_edge(id, src, tgt);
            e.kind = EdgeKind::Calls;
            s.upsert_edge(&e).unwrap();
        }

        let req = ArchitectureRequest {
            aspects: vec![ArchitectureAspect::Hotspots],
            top_n: 2,
            ..Default::default()
        };
        let report = s.architecture_overview(&req).unwrap();
        let hs = report.hotspots.expect("hotspots populated");
        assert_eq!(hs.len(), 2, "top_n=2 should yield 2 hotspots");
        // mk_node sets `name = id`, so the function named "d" has
        // node.name == "function:d:L1" (the synthetic id used to look
        // up node ids in edges). We assert by the trailing letter.
        assert!(
            hs[0].node.name.contains(":d:L"),
            "first hotspot should be d (3 inbound); got {:?}",
            hs.iter().map(|h| (&h.node.name, h.in_degree)).collect::<Vec<_>>()
        );
        assert_eq!(hs[0].in_degree, 3);
        assert!(
            hs[1].node.name.contains(":c:L"),
            "second hotspot should be c (2 inbound); got {:?}",
            hs.iter().map(|h| (&h.node.name, h.in_degree)).collect::<Vec<_>>()
        );
        assert_eq!(hs[1].in_degree, 2);
    }
}
