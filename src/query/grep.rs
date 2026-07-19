use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::path::Path;

use regex::Regex;

use crate::error::{GraphError, GraphResult};
use crate::storage::SqliteStorage;
use crate::types::*;

/// Search indexed source files and augment literal matches with graph context.
pub fn search_code(
    storage: &SqliteStorage,
    project_root: &Path,
    req: &SearchCodeRequest,
) -> GraphResult<SearchCodeResponse> {
    if req.pattern.is_empty() || req.pattern.split_whitespace().next().is_none() {
        return Ok(empty_response());
    }
    if req.limit == 0 {
        return Err(GraphError::Config(
            "search code limit must be at least 1".to_string(),
        ));
    }
    if req.context_lines > 50 {
        return Err(GraphError::Config(
            "search code context_lines must not exceed 50".to_string(),
        ));
    }

    let needles: Vec<&str> = req.pattern.split_whitespace().collect();
    let path_filter = req
        .path_filter
        .as_deref()
        .map(Regex::new)
        .transpose()
        .map_err(|error| GraphError::Config(format!("invalid path filter: {error}")))?;

    let mut file_paths = storage.all_file_paths()?;
    file_paths.sort();

    let mut total_grep_matches = 0;
    let mut grouped: HashMap<(Option<String>, String), SearchCodeMatch> = HashMap::new();

    for file_path in file_paths {
        if is_test_file(&file_path)
            || !has_allowed_extension(&file_path, req.file_extensions.as_deref())
            || !path_is_allowed(&file_path, path_filter.as_ref())
        {
            continue;
        }

        let content = match fs::read_to_string(project_root.join(&file_path)) {
            Ok(content) => content,
            Err(_) => continue,
        };
        let lines: Vec<&str> = content.lines().collect();

        for (line_index, source_line) in lines.iter().enumerate() {
            if source_line.chars().nth(4096).is_some()
                || !needles.iter().all(|needle| source_line.contains(needle))
            {
                continue;
            }

            total_grep_matches += 1;
            let line = line_index as u32 + 1;
            let col = source_line.find(needles[0]).unwrap_or_default() as u32;
            let containing_node = storage.containing_node_for_line(&file_path, line)?;
            let group_key = (
                containing_node.as_ref().map(|node| node.id.clone()),
                file_path.clone(),
            );

            match grouped.entry(group_key) {
                Entry::Occupied(mut entry) => entry.get_mut().match_count += 1,
                Entry::Vacant(entry) => {
                    let in_degree = match containing_node.as_ref() {
                        Some(node) => storage.in_degree_calls(&node.id)?,
                        None => 0,
                    };
                    let snippet = match req.mode {
                        SearchCodeMode::Compact => {
                            snippet_for(&lines, line_index, req.context_lines as usize)
                        }
                        SearchCodeMode::Files => String::new(),
                    };
                    let hit_kind = classify_hit(containing_node.as_ref());
                    entry.insert(SearchCodeMatch {
                        containing_node,
                        file_path: file_path.clone(),
                        line,
                        col,
                        snippet,
                        match_count: 1,
                        hit_kind,
                        in_degree,
                    });
                }
            }
        }
    }

    let mut results: Vec<SearchCodeMatch> = grouped.into_values().collect();
    results.sort_by(|left, right| {
        hit_rank(left.hit_kind)
            .cmp(&hit_rank(right.hit_kind))
            .then_with(|| right.in_degree.cmp(&left.in_degree))
            .then_with(|| left.file_path.cmp(&right.file_path))
            .then_with(|| left.line.cmp(&right.line))
    });

    let total_results = results.len();
    let truncated = total_results > req.limit;
    results.truncate(req.limit);

    Ok(SearchCodeResponse {
        total_grep_matches,
        total_results,
        results,
        truncated,
    })
}

fn empty_response() -> SearchCodeResponse {
    SearchCodeResponse {
        total_grep_matches: 0,
        total_results: 0,
        results: Vec::new(),
        truncated: false,
    }
}

fn has_allowed_extension(file_path: &str, extensions: Option<&[String]>) -> bool {
    let Some(extensions) = extensions else {
        return true;
    };
    extensions.iter().any(|extension| {
        let expected = extension.strip_prefix('.').unwrap_or(extension);
        let Some(suffix_start) = file_path.len().checked_sub(expected.len()) else {
            return false;
        };
        suffix_start > 0
            && file_path.as_bytes()[suffix_start - 1] == b'.'
            && file_path
                .get(suffix_start..)
                .is_some_and(|suffix| suffix.eq_ignore_ascii_case(expected))
    })
}

fn path_is_allowed(file_path: &str, filter: Option<&Regex>) -> bool {
    filter.is_none_or(|regex| {
        regex
            .find(file_path)
            .is_some_and(|matched| matched.start() == 0)
    })
}

fn is_test_file(file_path: &str) -> bool {
    let normalized = file_path.replace('\\', "/").to_ascii_lowercase();
    if normalized.starts_with("tests/") || normalized.contains("/tests/") {
        return true;
    }
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);
    file_name.ends_with("_test.rs")
        || file_name.ends_with(".test.ts")
        || file_name.ends_with(".spec.ts")
        || file_name.ends_with("_spec.rb")
}

fn snippet_for(lines: &[&str], line_index: usize, context_lines: usize) -> String {
    let start = line_index.saturating_sub(context_lines);
    let end = line_index
        .saturating_add(context_lines)
        .saturating_add(1)
        .min(lines.len());
    lines[start..end].join("\n")
}

fn classify_hit(node: Option<&Node>) -> HitKind {
    match node.map(|node| &node.kind) {
        Some(
            NodeKind::Function
            | NodeKind::Method
            | NodeKind::Class
            | NodeKind::Struct
            | NodeKind::Trait
            | NodeKind::Interface
            | NodeKind::Route,
        ) => HitKind::Definition,
        Some(NodeKind::Test) => HitKind::Test,
        _ => HitKind::Usage,
    }
}

fn hit_rank(kind: HitKind) -> u8 {
    match kind {
        HitKind::Definition => 0,
        HitKind::Usage => 1,
        HitKind::Test => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::SqliteStorage;
    use std::collections::HashMap;
    use std::fs;
    use std::path::Path;
    use tempfile::TempDir;

    fn fresh() -> (TempDir, SqliteStorage) {
        let dir = TempDir::new().expect("tempdir");
        let storage = SqliteStorage::open(&dir.path().join("graph.db")).expect("open db");
        (dir, storage)
    }

    fn index_file(storage: &SqliteStorage, root: &Path, path: &str, content: &str) {
        let full_path = root.join(path);
        fs::create_dir_all(full_path.parent().expect("file parent")).expect("create parent");
        fs::write(full_path, content).expect("write source");
        storage
            .upsert_file(&FileRecord {
                path: path.to_string(),
                language: Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .unwrap_or_default()
                    .to_string(),
                mtime: 0,
                content_hash: String::new(),
                indexed_at: 0,
            })
            .expect("index file");
    }

    fn mk_node(id: &str, file_path: &str, kind: NodeKind, start_line: u32, end_line: u32) -> Node {
        Node {
            id: id.to_string(),
            kind,
            name: id.to_string(),
            qualified_name: id.to_string(),
            file_path: file_path.to_string(),
            language: "rust".to_string(),
            start_line,
            end_line,
            start_column: 0,
            end_column: 0,
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

    fn request(pattern: &str) -> SearchCodeRequest {
        SearchCodeRequest {
            pattern: pattern.to_string(),
            regex: false,
            file_extensions: None,
            path_filter: None,
            mode: SearchCodeMode::Compact,
            context_lines: 1,
            limit: 50,
        }
    }

    #[test]
    fn dedup_hits_in_same_function() {
        let (dir, storage) = fresh();
        index_file(
            &storage,
            dir.path(),
            "src/a.rs",
            "needle needle\ncontext\nneedle\nline four\n",
        );
        storage
            .upsert_node(&mk_node(
                "function:a",
                "src/a.rs",
                NodeKind::Function,
                1,
                10,
            ))
            .unwrap();

        let response = search_code(&storage, dir.path(), &request("needle")).unwrap();

        assert_eq!(response.total_grep_matches, 2);
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].match_count, 2);
        assert_eq!(response.results[0].snippet, "needle needle\ncontext");
    }

    #[test]
    fn ranking_definition_before_test() {
        let (dir, storage) = fresh();
        index_file(
            &storage,
            dir.path(),
            "src/ranking.rs",
            "needle\nmiddle\nneedle\n",
        );
        storage
            .upsert_node(&mk_node(
                "function:definition",
                "src/ranking.rs",
                NodeKind::Function,
                1,
                1,
            ))
            .unwrap();
        storage
            .upsert_node(&mk_node(
                "test:case",
                "src/ranking.rs",
                NodeKind::Test,
                3,
                3,
            ))
            .unwrap();

        let response = search_code(&storage, dir.path(), &request("needle")).unwrap();

        assert_eq!(response.results.len(), 2);
        assert_eq!(
            response.results[0]
                .containing_node
                .as_ref()
                .map(|node| node.id.as_str()),
            Some("function:definition")
        );
    }

    #[test]
    fn path_filter_scoping() {
        let (dir, storage) = fresh();
        let source = format!("{}needle\n", "界".repeat(2_000));
        index_file(&storage, dir.path(), "src/a.rs", &source);
        index_file(&storage, dir.path(), "docs/b.rs", "needle\n");
        let mut req = request("needle");
        req.path_filter = Some("^src/".to_string());

        let response = search_code(&storage, dir.path(), &req).unwrap();

        assert_eq!(response.total_results, 1);
        assert_eq!(response.results[0].file_path, "src/a.rs");
    }

    #[test]
    fn in_degree_used_for_ranking() {
        let (dir, storage) = fresh();
        index_file(&storage, dir.path(), "src/a_cold.rs", "needle\n");
        index_file(&storage, dir.path(), "src/z_popular.rs", "needle\n");
        storage
            .upsert_node(&mk_node(
                "function:cold",
                "src/a_cold.rs",
                NodeKind::Function,
                1,
                1,
            ))
            .unwrap();
        storage
            .upsert_node(&mk_node(
                "function:popular",
                "src/z_popular.rs",
                NodeKind::Function,
                1,
                1,
            ))
            .unwrap();
        storage
            .upsert_edge(&mk_edge("call:1", "caller:1", "function:popular"))
            .unwrap();
        storage
            .upsert_edge(&mk_edge("call:2", "caller:2", "function:popular"))
            .unwrap();

        let response = search_code(&storage, dir.path(), &request("needle")).unwrap();

        assert_eq!(response.results[0].in_degree, 2);
        assert_eq!(
            response.results[0]
                .containing_node
                .as_ref()
                .map(|node| node.id.as_str()),
            Some("function:popular")
        );
    }

    #[test]
    fn files_mode_empty_snippet() {
        let (dir, storage) = fresh();
        index_file(&storage, dir.path(), "src/a.rs", "needle\n");
        let mut req = request("needle");
        req.mode = SearchCodeMode::Files;

        let response = search_code(&storage, dir.path(), &req).unwrap();

        assert_eq!(response.results.len(), 1);
        assert!(response.results[0].snippet.is_empty());
    }

    #[test]
    fn empty_pattern_returns_empty() {
        let (dir, storage) = fresh();

        let response = search_code(&storage, dir.path(), &request("")).unwrap();

        assert_eq!(response.total_grep_matches, 0);
        assert_eq!(response.total_results, 0);
        assert!(response.results.is_empty());
        assert!(!response.truncated);
    }

    #[test]
    fn file_extensions() {
        let (dir, storage) = fresh();
        index_file(&storage, dir.path(), "src/a.rs", "needle\n");
        index_file(&storage, dir.path(), "src/b.py", "needle\n");
        index_file(&storage, dir.path(), "src/types.D.TS", "needle\n");
        let mut req = request("needle");
        req.file_extensions = Some(vec!["rs".to_string()]);

        let response = search_code(&storage, dir.path(), &req).unwrap();

        assert_eq!(response.total_results, 1);
        assert_eq!(response.results[0].file_path, "src/a.rs");

        req.file_extensions = Some(vec!["d.ts".to_string()]);
        let response = search_code(&storage, dir.path(), &req).unwrap();
        assert_eq!(response.total_results, 1);
        assert_eq!(response.results[0].file_path, "src/types.D.TS");
    }
}
