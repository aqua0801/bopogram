//! Fixed-width records packed into 64-bit words.

/// `len` records of `width` bits each. One spare word at the end lets every
/// read take two words without a bounds check on the second.
pub struct BitArray {
    pub words: Vec<u64>,
    pub width: u32,
}

impl BitArray {
    pub fn new(len: usize, width: u32) -> Self {
        assert!(width > 0 && width <= 64);
        let bits = len as u64 * width as u64;
        BitArray { words: vec![0; (bits / 64) as usize + 2], width }
    }

    /// The field of `bits` bits at `offset` within record `index`.
    #[inline]
    pub fn get(&self, index: usize, offset: u32, bits: u32) -> u64 {
        let at = index as u64 * self.width as u64 + offset as u64;
        let word = (at / 64) as usize;
        let shift = (at % 64) as u32;
        let low = self.words[word] >> shift;
        let value = if shift == 0 { low } else { low | (self.words[word + 1] << (64 - shift)) };
        if bits == 64 {
            value
        } else {
            value & ((1u64 << bits) - 1)
        }
    }

    pub fn set(&mut self, index: usize, offset: u32, bits: u32, value: u64) {
        debug_assert!(bits == 64 || value < (1u64 << bits));
        let at = index as u64 * self.width as u64 + offset as u64;
        let word = (at / 64) as usize;
        let shift = (at % 64) as u32;
        let mask = if bits == 64 { u64::MAX } else { (1u64 << bits) - 1 };
        self.words[word] = (self.words[word] & !(mask << shift)) | (value << shift);
        if shift != 0 && shift + bits > 64 {
            let high = 64 - shift;
            self.words[word + 1] = (self.words[word + 1] & !(mask >> high)) | (value >> high);
        }
    }
}

/// Bits needed to store values up to `max`.
pub fn bits_for(max: u64) -> u32 {
    (64 - max.leading_zeros()).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_round_trip() {
        let mut a = BitArray::new(100, 37);
        for i in 0..100 {
            a.set(i, 0, 17, (i * 997 % 131_071) as u64);
            a.set(i, 17, 8, (i % 256) as u64);
            a.set(i, 25, 12, (i * 31 % 4096) as u64);
        }
        for i in 0..100 {
            assert_eq!(a.get(i, 0, 17), (i * 997 % 131_071) as u64);
            assert_eq!(a.get(i, 17, 8), (i % 256) as u64);
            assert_eq!(a.get(i, 25, 12), (i * 31 % 4096) as u64);
        }
        assert_eq!(bits_for(0), 1);
        assert_eq!(bits_for(255), 8);
        assert_eq!(bits_for(256), 9);
    }
}
