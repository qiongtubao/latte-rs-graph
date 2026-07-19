use std::collections::{HashMap, HashSet};

const INDEX_NON_ZERO: usize = 8;
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const SPLITMIX_INCREMENT: u64 = 0x9e37_79b9_7f4a_7c15;

pub struct RiCorpus {
    pub idf: HashMap<String, f32>,
    pub vec: HashMap<String, Vec<f32>>,
}

pub fn random_index_vec(token: &str, dim: usize) -> Vec<f32> {
    let mut vector = vec![0.0; dim];
    for_each_random_component(token, dim, |index, value| vector[index] = value);
    vector
}

pub fn build_ri_corpus<'a>(
    docs: impl IntoIterator<Item = &'a [String]>,
    window_size: usize,
    dim: usize,
) -> RiCorpus {
    let documents = docs.into_iter().collect::<Vec<_>>();
    let mut document_frequency: HashMap<&'a str, usize> = HashMap::new();

    for document in &documents {
        let mut seen = HashSet::new();
        for token in *document {
            seen.insert(token.as_str());
        }
        for token in seen {
            *document_frequency.entry(token).or_default() += 1;
        }
    }

    let document_count = documents.len() as f32;
    let idf = document_frequency
        .iter()
        .map(|(&token, &frequency)| {
            let weight = ((document_count + 1.0) / (frequency as f32 + 1.0)).ln() + 1.0;
            (token.to_owned(), weight)
        })
        .collect::<HashMap<_, _>>();

    // Accumulate in f64 so large corpora cannot overflow before normalization.
    let mut enriched = document_frequency
        .keys()
        .map(|&token| {
            let mut vector = vec![0.0_f64; dim];
            for_each_random_component(token, dim, |index, value| {
                vector[index] = f64::from(value);
            });
            (token, vector)
        })
        .collect::<HashMap<_, _>>();

    for document in &documents {
        for (index, token) in document.iter().enumerate() {
            let start = index.saturating_sub(window_size);
            let end = index
                .saturating_add(window_size)
                .saturating_add(1)
                .min(document.len());
            let target = enriched
                .get_mut(token.as_str())
                .expect("all document tokens have an RI vector");

            for (neighbor_index, neighbor) in document[start..end].iter().enumerate() {
                let neighbor_index = start + neighbor_index;
                if neighbor_index == index {
                    continue;
                }

                let weight = f64::from(
                    *idf.get(neighbor.as_str())
                        .expect("all document tokens have an IDF weight"),
                );
                for_each_random_component(neighbor, dim, |component, value| {
                    target[component] += weight * f64::from(value);
                });
            }
        }
    }

    let vec = enriched
        .into_iter()
        .map(|(token, vector)| {
            let squared_norm = vector.iter().map(|value| value * value).sum::<f64>();
            let normalized = if squared_norm > 0.0 && squared_norm.is_finite() {
                let inverse_norm = squared_norm.sqrt().recip();
                vector
                    .into_iter()
                    .map(|value| (value * inverse_norm) as f32)
                    .collect()
            } else {
                random_index_vec(token, dim)
            };
            (token.to_owned(), normalized)
        })
        .collect();

    RiCorpus { idf, vec }
}

fn for_each_random_component(token: &str, dim: usize, mut visit: impl FnMut(usize, f32)) {
    let component_count = dim.min(INDEX_NON_ZERO);
    if component_count == 0 {
        return;
    }

    let mut state = stable_hash(token.as_bytes());
    let mut position = next_splitmix(&mut state) as usize % dim;
    let step = if dim == 1 {
        0
    } else {
        let mut candidate = next_splitmix(&mut state) as usize % (dim - 1) + 1;
        while greatest_common_divisor(candidate, dim) != 1 {
            candidate = if candidate + 1 == dim {
                1
            } else {
                candidate + 1
            };
        }
        candidate
    };
    let invert_signs = next_splitmix(&mut state) & 1 != 0;
    let scale = (component_count as f32).sqrt().recip();
    let positive_count = component_count.div_ceil(2);

    for component in 0..component_count {
        let positive = (component < positive_count) != invert_signs;
        visit(position, if positive { scale } else { -scale });
        position = add_modulo(position, step, dim);
    }
}

fn stable_hash(bytes: &[u8]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    splitmix64(hash ^ bytes.len() as u64)
}

fn next_splitmix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(SPLITMIX_INCREMENT);
    splitmix64(*state)
}

fn splitmix64(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn greatest_common_divisor(mut left: usize, mut right: usize) -> usize {
    while right != 0 {
        let remainder = left % right;
        left = right;
        right = remainder;
    }
    left
}

fn add_modulo(value: usize, increment: usize, modulus: usize) -> usize {
    if increment == 0 {
        return value;
    }
    let distance_to_end = modulus - increment;
    if value >= distance_to_end {
        value - distance_to_end
    } else {
        value + increment
    }
}
