//! Deterministic PRNG (PCG32-XSH-RR) driving `rnd`/`srand`.
//!
//! Implemented inline so the core has zero non-Lua dependencies and produces
//! byte-identical streams on every target.

const MULT: u64 = 6_364_136_223_846_793_005;
/// Fixed stream selector: every console uses the same sequence, only the seed varies.
const STREAM: u64 = 1_442_695_040_888_963_407;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pcg32 {
    state: u64,
    inc: u64,
}

impl Pcg32 {
    /// Create a generator seeded with `seed`.
    pub fn new(seed: u64) -> Self {
        let mut rng = Pcg32 {
            state: 0,
            inc: (STREAM << 1) | 1,
        };
        rng.next_u32();
        rng.state = rng.state.wrapping_add(seed);
        rng.next_u32();
        rng
    }

    /// Reseed in place, exactly as if freshly constructed.
    pub fn reseed(&mut self, seed: u64) {
        *self = Pcg32::new(seed);
    }

    /// Next raw 32-bit output.
    pub fn next_u32(&mut self) -> u32 {
        let old = self.state;
        self.state = old.wrapping_mul(MULT).wrapping_add(self.inc);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        let rot = (old >> 59) as u32;
        xorshifted.rotate_right(rot)
    }

    /// Uniform float in `[0, 1)` with 32 bits of resolution.
    pub fn next_f64(&mut self) -> f64 {
        f64::from(self.next_u32()) / 4_294_967_296.0
    }
}

#[cfg(test)]
mod tests {
    use super::Pcg32;

    #[test]
    fn deterministic_and_seed_sensitive() {
        let a: Vec<u32> = (0..8).map(|_| Pcg32::new(7).next_u32()).collect();
        assert!(a.windows(2).all(|w| w[0] == w[1]), "fresh seeds must match");

        let mut r1 = Pcg32::new(7);
        let mut r2 = Pcg32::new(7);
        let mut r3 = Pcg32::new(8);
        let s1: Vec<u32> = (0..16).map(|_| r1.next_u32()).collect();
        let s2: Vec<u32> = (0..16).map(|_| r2.next_u32()).collect();
        let s3: Vec<u32> = (0..16).map(|_| r3.next_u32()).collect();
        assert_eq!(s1, s2);
        assert_ne!(s1, s3);
    }

    #[test]
    fn floats_in_unit_range() {
        let mut r = Pcg32::new(0);
        for _ in 0..1000 {
            let v = r.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
