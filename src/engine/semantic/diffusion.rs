//! Label-propagation diffusion for embedding vectors.
//!
//! [`diffuse`] performs a single iteration of label-propagation style blending:
//! each dimension of the input embedding is moved toward the mean of that
//! dimension across all "valid" neighbours, where a neighbour is simply any
//! vector with the same length as `embedding`. The blend factor `alpha` is
//! clamped to `[0, 1]`, the function is a no-op when the embedding is empty
//! or when no neighbour survives the length filter, and the result is
//! L2-normalized in place whenever its Euclidean norm is nonzero so cosine
//! similarity remains stable across iterations.
//!
//! The function is allocation-light: the input embedding is mutated in place
//! (never cloned), and the only transient allocation is a `mean` accumulator
//! sized to the embedding. No external dependencies are pulled in.

/// One label-propagation iteration over `embedding`.
///
/// For every dimension `d`, the function computes
/// `mean[d] = (sum of neighbour[d] over valid neighbours) / valid_count`,
/// then writes
/// `embedding[d] = (1 - alpha_clamped) * embedding[d] + alpha_clamped * mean[d]`
/// in place. After the blend, the embedding is L2-normalized — but only when
/// the resulting norm is finite and nonzero, so an all-zero or non-finite
/// result is left untouched (and any non-finite entries from degenerate
/// neighbours are zeroed out to keep the output well-defined).
///
/// Properties:
/// - Deterministic: depends only on the inputs, in order.
/// - Dimension-preserving: `embedding.len()` is unchanged.
/// - Robust: empty embedding, no valid neighbours, mismatched neighbour
///   lengths, and out-of-range `alpha` all degrade gracefully.
/// - Allocation-light: the input is mutated; the only allocation is a
///   `mean` buffer sized to the embedding dimension.
///
/// `alpha` is clamped into `[0, 1]`. A `NaN` alpha is treated as `0.0`
/// (the function leaves the embedding effectively unchanged after the blend
/// step, modulo normalization of the existing content).
///
/// Neighbours whose length differs from `embedding` are silently skipped.
/// If every neighbour is skipped, the function returns without modifying
/// `embedding`.
pub fn diffuse(embedding: &mut [f32], neighbor_embeddings: &[Vec<f32>], alpha: f32) {
    let dim = embedding.len();
    if dim == 0 {
        return;
    }

    // Clamp alpha into [0, 1]. NaN collapses to 0.0 so a poisoned alpha
    // degrades to "no blend" rather than producing non-finite output.
    let alpha = if alpha.is_nan() {
        0.0
    } else {
        alpha.clamp(0.0, 1.0)
    };

    // First pass: accumulate per-dimension sums and count valid neighbours.
    // `mean` doubles as the accumulator (initialised to zero) and as the
    // scratch buffer we write the per-dimension mean into before blending.
    let mut mean: Vec<f32> = vec![0.0_f32; dim];
    let mut valid: usize = 0;
    for neighbor in neighbor_embeddings {
        if neighbor.len() != dim {
            continue;
        }
        valid += 1;
        // `mean` is a local Vec and `neighbor` is an independent slice, so
        // there is no aliasing concern here.
        for d in 0..dim {
            mean[d] += neighbor[d];
        }
    }

    if valid == 0 {
        return;
    }

    let inv_count = 1.0_f32 / valid as f32;

    // Second pass: divide by count to get the per-dimension mean, then blend
    // into `embedding` in place. We mutate `embedding` directly — no clone.
    for d in 0..dim {
        let m = mean[d] * inv_count;
        let current = embedding[d];
        let blended = (1.0 - alpha) * current + alpha * m;
        embedding[d] = blended;
        // `mean` no longer needed; reuse slot to keep memory traffic down.
        mean[d] = blended;
    }

    // Sanitize: any NaN/inf from degenerate inputs collapses to 0. This
    // keeps the L2-norm well-defined and prevents NaN from propagating
    // through cosine similarity downstream.
    for d in 0..dim {
        if !mean[d].is_finite() {
            mean[d] = 0.0;
        }
    }

    // Third pass: compute squared norm of the blended, sanitized vector.
    let mut norm_sq = 0.0_f32;
    for d in 0..dim {
        let v = mean[d];
        norm_sq += v * v;
    }

    // Normalize only when the norm is finite AND strictly positive, so the
    // zero vector and non-finite edge cases stay untouched.
    if norm_sq.is_finite() && norm_sq > 0.0 {
        let norm = norm_sq.sqrt();
        // `norm` is finite and positive here, so division is safe.
        let inv_norm = 1.0_f32 / norm;
        for d in 0..dim {
            embedding[d] = mean[d] * inv_norm;
        }
    } else {
        // Norm is zero or non-finite — write the sanitized vector back
        // without scaling so the caller never sees NaN/inf in the output.
        for d in 0..dim {
            embedding[d] = mean[d];
        }
    }
}
