use crate::types::{SemanticConfig, SemanticSignature};
use std::collections::HashSet;

/// Blend the stored algorithmic signals into a bounded semantic similarity.
///
/// Each feature contributes in `[0, 1]`. The weighted blend is then moderated
/// by the structural boost: tests receive no boost, ordinary declarations are
/// neutral, and declarations sharing exported/async traits move toward one.
pub fn combined_score(a: &SemanticSignature, b: &SemanticSignature, cfg: &SemanticConfig) -> f32 {
    if a == b {
        return 1.0;
    }

    let tfidf = tfidf_cosine(&a.tokens, &b.tokens);
    let ri = cosine(&a.ri_vec, &b.ri_vec).max(0.0);
    let api = jaccard(&a.api_sig, &b.api_sig);
    let ast = cosine_or_equal_empty(&a.ast_profile, &b.ast_profile).max(0.0);
    let data_flow = jaccard(&a.data_flow_set, &b.data_flow_set);
    let module = module_proximity(a.module_path.as_deref(), b.module_path.as_deref());
    let minhash = minhash_similarity(&a.minhash, &b.minhash);
    let boost = struct_boost(a, b);

    let signals = [
        (cfg.weight_tfidf, tfidf),
        (cfg.weight_ri, ri),
        (cfg.weight_api, api),
        (cfg.weight_ast, ast),
        (cfg.weight_data_flow, data_flow),
        (cfg.weight_module_prox, module),
        (cfg.weight_minhash, minhash),
        (cfg.weight_struct_boost, boost),
    ];
    let mut weighted = 0.0;
    let mut weight_sum = 0.0;
    for (weight, signal) in signals {
        let weight = weight.max(0.0);
        weighted += weight * signal.clamp(0.0, 1.0);
        weight_sum += weight;
    }
    if weight_sum == 0.0 {
        return 0.0;
    }

    let base = weighted / weight_sum;
    (base * (1.0 + boost) * 0.5).clamp(0.0, 1.0)
}

fn tfidf_cosine(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let left = a.iter().map(String::as_str).collect::<HashSet<_>>();
    let right = b.iter().map(String::as_str).collect::<HashSet<_>>();
    let mut dot = 0.0_f32;
    let mut norm_left = 0.0_f32;
    let mut norm_right = 0.0_f32;
    for token in left.union(&right) {
        let token = *token;
        let in_left = left.contains(token);
        let in_right = right.contains(token);
        let document_frequency = usize::from(in_left) + usize::from(in_right);
        let idf = (3.0_f32 / (document_frequency as f32 + 1.0)).ln() + 1.0;
        let weighted_left = if in_left { idf } else { 0.0 };
        let weighted_right = if in_right { idf } else { 0.0 };
        dot += weighted_left * weighted_right;
        norm_left += weighted_left * weighted_left;
        norm_right += weighted_right * weighted_right;
    }
    if norm_left == 0.0 || norm_right == 0.0 {
        0.0
    } else {
        dot / (norm_left.sqrt() * norm_right.sqrt())
    }
}

fn jaccard(a: &[String], b: &[String]) -> f32 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let left = a.iter().map(String::as_str).collect::<HashSet<_>>();
    let right = b.iter().map(String::as_str).collect::<HashSet<_>>();
    let union = left.union(&right).count();
    if union == 0 {
        1.0
    } else {
        left.intersection(&right).count() as f32 / union as f32
    }
}

fn cosine_or_equal_empty(a: &[f32], b: &[f32]) -> f32 {
    if a.iter().all(|value| *value == 0.0) && b.iter().all(|value| *value == 0.0) {
        1.0
    } else {
        cosine(a, b)
    }
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut norm_a, mut norm_b) = (0.0_f64, 0.0_f64, 0.0_f64);
    for (&left, &right) in a.iter().zip(b) {
        if !left.is_finite() || !right.is_finite() {
            return 0.0;
        }
        let left = f64::from(left);
        let right = f64::from(right);
        dot += left * right;
        norm_a += left * left;
        norm_b += right * right;
    }
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a.sqrt() * norm_b.sqrt())) as f32
}

fn minhash_similarity(a: &[u32], b: &[u32]) -> f32 {
    let compared = a.len().min(b.len());
    if compared == 0 {
        return if a.is_empty() && b.is_empty() {
            1.0
        } else {
            0.0
        };
    }
    let equal = a
        .iter()
        .zip(b)
        .take(compared)
        .filter(|(left, right)| left == right)
        .count();
    equal as f32 / compared as f32
}

fn module_proximity(a: Option<&str>, b: Option<&str>) -> f32 {
    let (Some(a), Some(b)) = (a, b) else {
        return 0.5;
    };
    let left = a
        .split(['/', ':'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let right = b
        .split(['/', ':'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let total = left.len().max(right.len());
    if total == 0 {
        return 1.0;
    }
    let common = left.iter().zip(&right).take_while(|(x, y)| x == y).count();
    common as f32 / total as f32
}

fn struct_boost(a: &SemanticSignature, b: &SemanticSignature) -> f32 {
    if is_test_name(&a.name) || is_test_name(&b.name) {
        return 0.0;
    }
    if (a.is_exported && b.is_exported) || (a.is_async && b.is_async) {
        1.0
    } else if a.is_exported == b.is_exported && a.is_async == b.is_async {
        0.5
    } else {
        0.25
    }
}

fn is_test_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("test_")
        || name.starts_with("it_")
        || name.ends_with("_test")
        || name.contains("::test_")
}
