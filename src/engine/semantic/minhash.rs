const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
const SEED_BASIS: u64 = 0x6a09_e667_f3bc_c909;
const SEED_INCREMENT: u64 = 0x9e37_79b9_7f4a_7c15;
const EMPTY_SHINGLE: u64 = 0x3c6e_f372_fe94_f82b;

pub fn compute_minhash(tokens: &[String], k: usize) -> Vec<u32> {
    if k == 0 {
        return Vec::new();
    }

    let shingle_hashes = match tokens.len() {
        0 => vec![EMPTY_SHINGLE],
        1 | 2 => vec![hash_token_sequence(tokens)],
        _ => tokens
            .windows(3)
            .map(hash_token_sequence)
            .collect::<Vec<_>>(),
    };

    (0..k)
        .map(|seed_index| {
            let seed = splitmix64(
                SEED_BASIS.wrapping_add((seed_index as u64).wrapping_mul(SEED_INCREMENT)),
            );
            shingle_hashes
                .iter()
                .map(|&shingle| fold_to_u32(splitmix64(shingle ^ seed)))
                .min()
                .expect("every token sequence has at least one shingle")
        })
        .collect()
}

fn hash_token_sequence(tokens: &[String]) -> u64 {
    let mut hash = FNV_OFFSET_BASIS;
    hash_u64(&mut hash, tokens.len() as u64);
    for token in tokens {
        hash_u64(&mut hash, token.len() as u64);
        for &byte in token.as_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(FNV_PRIME);
        }
    }
    splitmix64(hash)
}

fn hash_u64(hash: &mut u64, value: u64) {
    for byte in value.to_le_bytes() {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(FNV_PRIME);
    }
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(SEED_INCREMENT);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn fold_to_u32(value: u64) -> u32 {
    (value ^ (value >> 32)) as u32
}
