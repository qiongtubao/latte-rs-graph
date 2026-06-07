use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use crate::error::{GraphError, GraphResult};
use crate::types::*;

/// SQLite-backed graph storage, aligned with @latte-graph/core schema.
pub struct SqliteStorage {
    conn: Mutex<Connection>,
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

    // =====================================================================
    // Write
    // =====================================================================

    pub fn upsert_node(&self, node: &Node) -> GraphResult<()> {
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
        Ok(())
    }

    pub fn upsert_nodes_batch(&self, nodes: &[Node]) -> GraphResult<()> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let tx = conn.unchecked_transaction()?;
        for node in nodes {
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
        conn.execute_batch("DELETE FROM nodes; DELETE FROM edges; DELETE FROM files; DELETE FROM `references`;")?;
        Ok(())
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

    pub fn search_nodes(&self, query: &str, limit: usize) -> GraphResult<Vec<Node>> {
        let conn = self.conn.lock().map_err(|e| GraphError::Engine(e.to_string()))?;
        let pattern = format!("%{}%", query);
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, qualified_name, file_path, language,
                    start_line, end_line, start_column, end_column,
                    signature, docstring, visibility,
                    is_exported, is_async, is_static, is_abstract, extra
             FROM nodes WHERE valid = 1
               AND (name LIKE ?1 OR qualified_name LIKE ?1 OR file_path LIKE ?1)
             LIMIT ?2",
        )?;

        let rows = stmt.query_map(params![pattern, limit as i64], |row| {
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
        Ok(nodes)
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

        let rows = stmt.query_map(params![name], |row| {
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

        let mut rows = stmt.query_map(params![id], |row| {
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
}
