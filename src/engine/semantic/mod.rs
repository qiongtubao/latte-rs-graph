pub mod ast_profile;
pub mod combiner;
pub mod diffusion;
pub mod minhash;
pub mod ri;
pub mod tokenize;
pub mod types;

pub use ast_profile::compute_ast_profile;
pub use combiner::combined_score;
pub use minhash::compute_minhash;
pub use ri::random_index_vec;
pub use tokenize::semantic_tokens;
pub use types::{SemanticConfig, SemanticSignature};

use std::collections::HashMap;

const DEFAULT_DIM: usize = 768;
const MINHASH_SIZE: usize = 64;

/// Compute every local algorithmic signal for one callable AST subtree.
pub fn embed_function(
    source: &[u8],
    func_root: tree_sitter::Node,
    func_name: &str,
    lang: &str,
) -> SemanticSignature {
    let tokens = semantic_tokens(source, func_root);
    let ast_profile = compute_ast_profile(source, func_root, lang);
    let minhash = compute_minhash(&tokens, MINHASH_SIZE);
    let data_flow_set = tokenize::data_flow_tokens(source, func_root);
    let api_sig = tokenize::api_signature(source, func_root);
    let corpus = ri::build_ri_corpus(std::iter::once(tokens.as_slice()), 5, DEFAULT_DIM);

    let ri_vec = corpus_ri_vec(&tokens, &corpus, DEFAULT_DIM);

    // RI carries lexical/co-occurrence identity. Projecting the compact AST
    // and MinHash channels into stable dimensions makes the stored embedding
    // useful by itself while the combiner retains each original signal.
    let mut embedding = ri_vec.clone();
    let ast_norm = ast_profile
        .iter()
        .map(|value| value.abs())
        .fold(0.0_f32, f32::max)
        .max(1.0);
    for (index, value) in ast_profile.iter().enumerate() {
        embedding[(index * 31 + 17) % DEFAULT_DIM] += 0.15 * (*value / ast_norm);
    }
    for (index, hash) in minhash.iter().enumerate() {
        let position = (*hash as usize) % DEFAULT_DIM;
        let sign = if hash & 1 == 0 { 1.0 } else { -1.0 };
        embedding[position] += sign * (0.10 / MINHASH_SIZE as f32);
        // Salt repeated positions so the 64 permutations do not collapse.
        embedding[(position + index * 13) % DEFAULT_DIM] += sign * (0.05 / MINHASH_SIZE as f32);
    }
    normalize(&mut embedding);

    let declaration = func_root.utf8_text(source).unwrap_or_default();
    let is_exported = declaration
        .split_whitespace()
        .take(4)
        .any(|word| matches!(word, "pub" | "export" | "public"));
    let is_async = declaration
        .split_whitespace()
        .take(4)
        .any(|word| word == "async");

    SemanticSignature {
        name: func_name.to_string(),
        tokens,
        ast_profile,
        minhash,
        data_flow_set,
        api_sig,
        ri_vec,
        embedding,
        module_path: None,
        is_exported,
        is_async,
    }
}

pub(crate) fn corpus_ri_vec(tokens: &[String], corpus: &ri::RiCorpus, dim: usize) -> Vec<f32> {
    let mut combined = vec![0.0; dim];
    for token in tokens {
        let weight = corpus.idf.get(token).copied().unwrap_or(1.0);
        if let Some(vector) = corpus.vec.get(token) {
            for (slot, value) in combined.iter_mut().zip(vector) {
                *slot += weight * value;
            }
        }
    }
    normalize(&mut combined);
    combined
}

/// Flatten a signature into the string-valued `Node::extra` representation.
pub fn signature_to_extra(signature: &SemanticSignature) -> HashMap<String, String> {
    let mut extra = HashMap::with_capacity(14);
    extra.insert("semantic.dim".into(), signature.embedding.len().to_string());
    extra.insert("semantic.name".into(), signature.name.clone());
    extra.insert("semantic.tokens".into(), signature.tokens.join(" "));
    extra.insert("semantic.body_n".into(), signature.tokens.len().to_string());
    extra.insert(
        "semantic.ast_profile".into(),
        join_floats(&signature.ast_profile),
    );
    extra.insert(
        "semantic.minhash".into(),
        signature
            .minhash
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" "),
    );
    extra.insert(
        "semantic.data_flow".into(),
        signature.data_flow_set.join(" "),
    );
    extra.insert("semantic.api_sig".into(), signature.api_sig.join(" "));
    extra.insert("semantic.ri".into(), join_floats(&signature.ri_vec));
    extra.insert(
        "semantic.embedding".into(),
        join_floats(&signature.embedding),
    );
    extra.insert("semantic.method".into(), "algorithmic".into());
    extra.insert(
        "semantic.module_path".into(),
        signature.module_path.clone().unwrap_or_default(),
    );
    extra.insert(
        "semantic.is_exported".into(),
        signature.is_exported.to_string(),
    );
    extra.insert("semantic.is_async".into(), signature.is_async.to_string());
    extra
}

/// Reconstruct a signature from `Node::extra`; malformed embeddings are skipped.
pub fn signature_from_extra(extra: &HashMap<String, String>) -> Option<SemanticSignature> {
    if extra.get("semantic.method")?.as_str() != "algorithmic" {
        return None;
    }
    let embedding = parse_floats(extra.get("semantic.embedding")?)?;
    let declared_dim = extra.get("semantic.dim")?.parse::<usize>().ok()?;
    if embedding.len() != declared_dim || declared_dim == 0 {
        return None;
    }
    let ri_vec = extra
        .get("semantic.ri")
        .and_then(|value| parse_floats(value))
        .filter(|vector| vector.len() == declared_dim)
        .unwrap_or_else(|| embedding.clone());

    Some(SemanticSignature {
        name: extra.get("semantic.name").cloned().unwrap_or_default(),
        tokens: split_words(extra.get("semantic.tokens")),
        ast_profile: parse_floats(extra.get("semantic.ast_profile")?)?,
        minhash: extra
            .get("semantic.minhash")?
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<Vec<u32>, _>>()
            .ok()?,
        data_flow_set: split_words(extra.get("semantic.data_flow")),
        api_sig: split_words(extra.get("semantic.api_sig")),
        ri_vec,
        embedding,
        module_path: extra
            .get("semantic.module_path")
            .filter(|path| !path.is_empty())
            .cloned(),
        is_exported: extra
            .get("semantic.is_exported")
            .is_some_and(|value| value == "true"),
        is_async: extra
            .get("semantic.is_async")
            .is_some_and(|value| value == "true"),
    })
}

fn normalize(vector: &mut [f32]) {
    let squared_norm = vector
        .iter()
        .map(|value| f64::from(*value) * f64::from(*value))
        .sum::<f64>();
    if squared_norm > 0.0 && squared_norm.is_finite() {
        let inverse = squared_norm.sqrt().recip() as f32;
        for value in vector {
            *value *= inverse;
        }
    }
}

fn join_floats(values: &[f32]) -> String {
    values
        .iter()
        .map(|value| format!("{value:.6}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_floats(value: &str) -> Option<Vec<f32>> {
    value
        .split_whitespace()
        .map(str::parse::<f32>)
        .collect::<Result<Vec<_>, _>>()
        .ok()
        .filter(|values| values.iter().all(|value| value.is_finite()))
}

fn split_words(value: Option<&String>) -> Vec<String> {
    value
        .map(|words| words.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::TreeSitterEngine;
    use crate::storage::SqliteStorage;
    use crate::traits::GraphProvider;
    use crate::types::{BuildOptions, EdgeKind};
    use std::fs;
    use tempfile::TempDir;
    use tree_sitter::Parser;

    fn rust_root(source: &str) -> tree_sitter::Tree {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .unwrap();
        parser.parse(source, None).unwrap()
    }

    #[test]
    fn semantic_tokens_extracts_rust_identifiers() {
        let source = "fn compute_total(items: Vec<i32>) -> i32 { let mut sum = 0; for x in items { sum = sum + x; } sum }";
        let tree = rust_root(source);
        let function = tree.root_node().child(0).unwrap();

        let tokens = semantic_tokens(source.as_bytes(), function);

        assert!(tokens.contains(&"items".to_string()));
        assert!(tokens.contains(&"sum".to_string()));
        assert!(!tokens.contains(&"fn".to_string()));
    }

    #[test]
    fn ast_profile_25_dimensions() {
        let source = "fn choose(value: i32) -> i32 { if value > 0 { value } else { 0 } }";
        let tree = rust_root(source);
        let function = tree.root_node().child(0).unwrap();

        let profile = compute_ast_profile(source.as_bytes(), function, "rust");

        assert_eq!(profile.len(), 25);
    }

    #[test]
    fn minhash_returns_64_shingles() {
        let tokens = ["a", "b", "c", "d", "e", "f"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        assert_eq!(compute_minhash(&tokens, 64).len(), 64);
    }

    #[test]
    fn random_index_vec_is_deterministic_and_sparse() {
        let first = random_index_vec("foo", 768);
        let second = random_index_vec("foo", 768);
        let non_zero = first.iter().filter(|value| **value != 0.0).count();

        assert_eq!(first, second);
        assert!((3..=12).contains(&non_zero), "non-zero count: {non_zero}");
    }

    #[test]
    fn combined_score_self_is_one() {
        let source = "pub async fn sum(items: Vec<i32>) -> i32 { items.iter().sum() }";
        let tree = rust_root(source);
        let function = tree.root_node().child(0).unwrap();
        let signature = embed_function(source.as_bytes(), function, "sum", "rust");

        let score = combined_score(&signature, &signature, &SemanticConfig::default());

        assert!((score - 1.0).abs() < f32::EPSILON, "score: {score}");
    }

    #[test]
    fn combined_score_different_is_lower() {
        let first_source = "pub async fn sum(items: Vec<i32>) -> i32 { items.iter().sum() }";
        let first_tree = rust_root(first_source);
        let first = embed_function(
            first_source.as_bytes(),
            first_tree.root_node().child(0).unwrap(),
            "sum",
            "rust",
        );
        let second_source = "fn render(title: String) -> String { format!(\"<h1>{title}</h1>\") }";
        let second_tree = rust_root(second_source);
        let second = embed_function(
            second_source.as_bytes(),
            second_tree.root_node().child(0).unwrap(),
            "render",
            "rust",
        );
        let config = SemanticConfig::default();

        assert!(combined_score(&first, &second, &config) < combined_score(&first, &first, &config));
    }

    #[tokio::test]
    async fn emit_semantic_edges_after_build_creates_links() {
        let dir = TempDir::new().unwrap();
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(
            src_dir.join("a.rs"),
            "pub fn total_values(values: &[i32]) -> i32 { values.iter().copied().sum() }",
        )
        .unwrap();
        fs::write(
            src_dir.join("b.rs"),
            "pub fn aggregate_values(values: &[i32]) -> i32 { values.iter().copied().sum() }",
        )
        .unwrap();
        let storage = SqliteStorage::open(&dir.path().join("semantic.db")).unwrap();
        let engine = TreeSitterEngine::new(storage);
        let options = BuildOptions {
            project_root: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };

        engine.build(dir.path(), &options).await.unwrap();
        let graph = engine.graph_data().await.unwrap();

        assert!(
            graph
                .edges
                .iter()
                .any(|edge| { edge.kind == EdgeKind::Other("semantically_related".to_string()) })
        );
    }
}
