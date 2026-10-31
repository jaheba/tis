//! CPython-compatible MT19937 integer seeding and word packing, adapted from Monty.
#[derive(Debug, Clone)]
pub struct Mt19937 {
    state: Vec<u32>,
    index: usize,
}
impl Mt19937 {
    pub fn new(key: &[u32]) -> Self {
        let key = if key.is_empty() { &[0][..] } else { key };
        let mut state = vec![0u32; 624];
        state[0] = 19_650_218;
        for i in 1..624 {
            state[i] = 1_812_433_253u32
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 30))
                .wrapping_add(i as u32);
        }
        let (mut i, mut j) = (1usize, 0usize);
        for _ in 0..624.max(key.len()) {
            state[i] = (state[i] ^ (state[i - 1] ^ (state[i - 1] >> 30)).wrapping_mul(1_664_525))
                .wrapping_add(key[j])
                .wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i == 624 {
                state[0] = state[623];
                i = 1;
            }
            if j == key.len() {
                j = 0;
            }
        }
        for _ in 0..623 {
            state[i] = (state[i]
                ^ (state[i - 1] ^ (state[i - 1] >> 30)).wrapping_mul(1_566_083_941))
            .wrapping_sub(i as u32);
            i += 1;
            if i == 624 {
                state[0] = state[623];
                i = 1;
            }
        }
        state[0] = 0x8000_0000;
        Self { state, index: 624 }
    }
    pub fn word(&mut self) -> u32 {
        if self.index == 624 {
            for i in 0..624 {
                let next = if i == 623 { 0 } else { i + 1 };
                let y = (self.state[i] & 0x8000_0000) | (self.state[next] & 0x7fff_ffff);
                self.state[i] = self.state[(i + 397) % 624]
                    ^ (y >> 1)
                    ^ if y & 1 != 0 { 0x9908_b0df } else { 0 };
            }
            self.index = 0;
        }
        let mut y = self.state[self.index];
        self.index += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }
    pub fn random(&mut self) -> f64 {
        let a = self.word() >> 5;
        let b = self.word() >> 6;
        (f64::from(a) * 67_108_864.0 + f64::from(b)) * (1.0 / 9_007_199_254_740_992.0)
    }
    pub fn bits(&mut self, bits: usize) -> num_bigint::BigInt {
        let mut words = Vec::with_capacity(bits.div_ceil(32));
        let mut left = bits;
        while left != 0 {
            let take = left.min(32);
            words.push(self.word() >> (32 - take));
            left -= take;
        }
        num_bigint::BigInt::from_biguint(num_bigint::Sign::Plus, num_bigint::BigUint::new(words))
    }
    pub fn bytes(&mut self, length: usize) -> Vec<u8> {
        let mut data = Vec::with_capacity(length);
        for _ in 0..length / 4 {
            data.extend_from_slice(&self.word().to_le_bytes());
        }
        let tail = length % 4;
        if tail != 0 {
            let word = self.word() >> (32 - tail * 8);
            data.extend_from_slice(&word.to_le_bytes()[..tail]);
        }
        data
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integer_seed_matches_cpython() {
        let mut rng = Mt19937::new(&[0]);
        assert_eq!(rng.random(), 0.8444218515250481);
        assert_eq!(rng.random(), 0.7579544029403025);
        let mut rng = Mt19937::new(&[12345]);
        assert_eq!(rng.random(), 0.41661987254534116);
    }
}
