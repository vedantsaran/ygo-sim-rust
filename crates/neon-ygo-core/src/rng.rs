use serde::{Deserialize, Serialize};

/// Exact Mersenne Twister MT19937 PRNG compatible with YGOPro / EDOPro / YGO Omega `.yrp` replays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mt19937 {
    mt: Vec<u32>,
    index: usize,
    seed: u32,
}

const N: usize = 624;
const M: usize = 397;
const MATRIX_A: u32 = 0x9908_b0df;
const UPPER_MASK: u32 = 0x8000_0000;
const LOWER_MASK: u32 = 0x7fff_ffff;

impl Mt19937 {
    pub fn new(seed: u32) -> Self {
        let mut mt = vec![0u32; N];
        mt[0] = seed;
        for i in 1..N {
            let prev = mt[i - 1];
            mt[i] = 1812433253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
        Self { mt, index: N, seed }
    }

    pub fn from_seed_array(seeds: &[u32; 4]) -> Self {
        let mut rng = Self::new(19650218);
        let mut i = 1usize;
        let mut j = 0usize;
        let mut k = N.max(seeds.len());
        while k > 0 {
            let prev = rng.mt[i - 1];
            rng.mt[i] = (rng.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1664525))
                .wrapping_add(seeds[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= N {
                rng.mt[0] = rng.mt[N - 1];
                i = 1;
            }
            if j >= seeds.len() {
                j = 0;
            }
            k -= 1;
        }
        for _ in 0..(N - 1) {
            let prev = rng.mt[i - 1];
            rng.mt[i] = (rng.mt[i] ^ (prev ^ (prev >> 30)).wrapping_mul(1566083941))
                .wrapping_sub(i as u32);
            i += 1;
            if i >= N {
                rng.mt[0] = rng.mt[N - 1];
                i = 1;
            }
        }
        rng.mt[0] = 0x8000_0000;
        rng.seed = seeds[0];
        rng
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    fn twist(&mut self) {
        for i in 0..N {
            let x = (self.mt[i] & UPPER_MASK) | (self.mt[(i + 1) % N] & LOWER_MASK);
            let mut xa = x >> 1;
            if x & 1 != 0 {
                xa ^= MATRIX_A;
            }
            self.mt[i] = self.mt[(i + M) % N] ^ xa;
        }
        self.index = 0;
    }

    pub fn next_u32(&mut self) -> u32 {
        if self.index >= N {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^= y >> 18;
        y
    }

    /// Inclusive range `[low, high]`.
    pub fn next_range(&mut self, low: u32, high: u32) -> u32 {
        if low >= high {
            return low;
        }
        let span = (high - low) as u64 + 1;
        low + ((self.next_u32() as u64) % span) as u32
    }

    /// Fisher-Yates shuffle matching ocgcore.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        if slice.len() <= 1 {
            return;
        }
        for i in (1..slice.len()).rev() {
            let j = self.next_range(0, i as u32) as usize;
            slice.swap(i, j);
        }
    }

    /// Coin toss: returns `1` for Heads, `0` for Tails.
    pub fn toss_coin(&mut self) -> u8 {
        (self.next_u32() & 1) as u8
    }

    /// 6-sided die roll: returns `1..=6`.
    pub fn roll_dice(&mut self) -> u8 {
        self.next_range(1, 6) as u8
    }
}
