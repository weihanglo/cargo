//! Implementation of a hasher that produces the same values across releases.

pub use self::imp::StableHasher;

#[cfg(not(any(feature = "stable-hash-siphash", feature = "stable-hash-blake3")))]
mod imp {
    #![allow(deprecated)]

    use std::hash::Hasher;
    use std::hash::SipHasher;

    pub struct StableHasher(SipHasher);

    impl StableHasher {
        pub fn new() -> StableHasher {
            StableHasher(SipHasher::new())
        }
    }

    impl Hasher for StableHasher {
        fn finish(&self) -> u64 {
            self.0.finish()
        }
        fn write(&mut self, bytes: &[u8]) {
            self.0.write(bytes)
        }
    }
}

#[cfg(all(not(feature = "stable-hash-siphash"), feature = "stable-hash-blake3"))]
mod imp {
    use std::hash::Hasher;

    use rustc_stable_hash::ExtendedHasher;

    #[derive(Debug, Clone)]
    pub struct StableHasher {
        state: blake3::Hasher,
    }

    impl StableHasher {
        pub fn new() -> StableHasher {
            StableHasher {
                state: Default::default(),
            }
        }
    }

    impl ExtendedHasher for StableHasher {
        type Hash = blake3::Hash;

        #[inline]
        fn finish(self) -> Self::Hash {
            self.state.finalize()
        }
    }

    impl Hasher for StableHasher {
        #[inline]
        fn write(&mut self, bytes: &[u8]) {
            self.state.update(bytes);
        }

        #[inline]
        fn finish(&self) -> u64 {
            let hash = self.state.finalize();
            let [a0, a1, a2, a3, a4, a5, a6, a7, b0, b1, b2, b3, b4, b5, b6, b7, c0, c1, c2, c3, c4, c5, c6, c7, d0, d1, d2, d3, d4, d5, d6, d7] =
                *hash.as_bytes();
            let p0 = u64::from_ne_bytes([a0, a1, a2, a3, a4, a5, a6, a7]);
            let p1 = u64::from_ne_bytes([b0, b1, b2, b3, b4, b5, b6, b7]);
            let p2 = u64::from_ne_bytes([c0, c1, c2, c3, c4, c5, c6, c7]);
            let p3 = u64::from_ne_bytes([d0, d1, d2, d3, d4, d5, d6, d7]);
            p0.wrapping_mul(3)
                .wrapping_add(p1)
                .wrapping_add(p2)
                .wrapping_mul(p3)
                .to_le()
        }
    }
}

#[cfg(all(feature = "stable-hash-siphash", not(feature = "stable-hash-blake3")))]
mod imp {
    pub use rustc_stable_hash::StableSipHasher128 as StableHasher;
}

#[cfg(all(feature = "stable-hash-siphash", feature = "stable-hash-blake3"))]
compile_error!("must choose only one of `siphash` or `blake3`");
