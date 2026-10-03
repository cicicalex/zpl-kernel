//! 8 ZPL bitwise operators over 64x64 binary matrices, with both a
//! naive byte loop and an SSE2 16-byte SIMD path.
//!
//! Operators (commutative bitwise on byte streams, treating each bit as a
//! cell of the matrix):
//!
//! - XOR  = a ^ b
//! - AND  = a & b
//! - OR   = a | b
//! - NAND = !(a & b)
//! - NOR  = !(a | b)
//! - XNOR = !(a ^ b)
//! - IMP  = !a | b   (logical implication)
//! - NIMP =  a & !b
//!
//! Inputs are flat byte arrays of length 64*64/8 = 512 bytes (one bit per
//! cell). The SIMD path uses `core::arch::x86_64::__m128i` to fan out to
//! 16-byte XMM registers (SSE2 only — present on every x86_64 CPU).
//!
//! Acceptance:
//! - Self-check verifies SIMD output is byte-identical with the naive
//!   loop on a known input pair (correctness gate).
//! - The throughput target ("≥ 10x naive loop") is measured separately
//!   in the per-syscall benchmark suite; this module owns
//!   correctness, not micro-benchmarking.

#![cfg(all(target_os = "none", target_arch = "x86_64"))]

use core::arch::x86_64::{
    _mm_and_si128, _mm_loadu_si128, _mm_or_si128, _mm_set1_epi8, _mm_storeu_si128, _mm_xor_si128,
    __m128i,
};

pub const MATRIX_BYTES: usize = 64 * 64 / 8; // = 512 bytes

#[derive(Clone, Copy)]
pub enum Op {
    Xor,
    And,
    Or,
    Nand,
    Nor,
    Xnor,
    Imp,
    Nimp,
}

/// Naive byte-by-byte implementation of all 8 operators. Used as the
/// reference for the SIMD path.
pub fn naive(op: Op, a: &[u8; MATRIX_BYTES], b: &[u8; MATRIX_BYTES]) -> [u8; MATRIX_BYTES] {
    let mut out = [0u8; MATRIX_BYTES];
    for i in 0..MATRIX_BYTES {
        out[i] = match op {
            Op::Xor => a[i] ^ b[i],
            Op::And => a[i] & b[i],
            Op::Or => a[i] | b[i],
            Op::Nand => !(a[i] & b[i]),
            Op::Nor => !(a[i] | b[i]),
            Op::Xnor => !(a[i] ^ b[i]),
            Op::Imp => !a[i] | b[i],
            Op::Nimp => a[i] & !b[i],
        };
    }
    out
}

/// SSE2 16-byte SIMD implementation. Returns a freshly-built array.
pub fn simd(op: Op, a: &[u8; MATRIX_BYTES], b: &[u8; MATRIX_BYTES]) -> [u8; MATRIX_BYTES] {
    let mut out = [0u8; MATRIX_BYTES];
    // SAFETY: target is x86_64; SSE2 is unconditionally available; the
    // pointer math stays inside the 512-byte arrays we own.
    unsafe {
        let ones = _mm_set1_epi8(-1i8); // all-1s for bitwise NOT
        let mut i = 0usize;
        while i + 16 <= MATRIX_BYTES {
            let va = _mm_loadu_si128(a.as_ptr().add(i) as *const __m128i);
            let vb = _mm_loadu_si128(b.as_ptr().add(i) as *const __m128i);
            let result = match op {
                Op::Xor => _mm_xor_si128(va, vb),
                Op::And => _mm_and_si128(va, vb),
                Op::Or => _mm_or_si128(va, vb),
                Op::Nand => _mm_xor_si128(_mm_and_si128(va, vb), ones),
                Op::Nor => _mm_xor_si128(_mm_or_si128(va, vb), ones),
                Op::Xnor => _mm_xor_si128(_mm_xor_si128(va, vb), ones),
                Op::Imp => _mm_or_si128(_mm_xor_si128(va, ones), vb),
                Op::Nimp => _mm_and_si128(va, _mm_xor_si128(vb, ones)),
            };
            _mm_storeu_si128(out.as_mut_ptr().add(i) as *mut __m128i, result);
            i += 16;
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfCheckErr {
    Mismatch { op: u8 },
}

pub fn self_check() -> Result<(), SelfCheckErr> {
    // Two pseudorandom but deterministic inputs derived from a
    // constant-time bit-mixer sweep so we exercise every byte position
    // with mixed 0/1 cells.
    let mut a = [0u8; MATRIX_BYTES];
    let mut b = [0u8; MATRIX_BYTES];
    let mut state_a: u64 = 0xA5A5_A5A5_A5A5_A5A5;
    let mut state_b: u64 = 0x5A5A_5A5A_5A5A_5A5A;
    for i in 0..MATRIX_BYTES {
        a[i] = (next(&mut state_a) & 0xFF) as u8;
        b[i] = (next(&mut state_b) & 0xFF) as u8;
    }

    let ops = [
        (0u8, Op::Xor),
        (1, Op::And),
        (2, Op::Or),
        (3, Op::Nand),
        (4, Op::Nor),
        (5, Op::Xnor),
        (6, Op::Imp),
        (7, Op::Nimp),
    ];

    for (id, op) in ops.iter() {
        let n = naive(*op, &a, &b);
        let s = simd(*op, &a, &b);
        if n != s {
            return Err(SelfCheckErr::Mismatch { op: *id });
        }
    }
    Ok(())
}

fn next(state: &mut u64) -> u64 {
    let mut z = state.wrapping_add(0x9E3779B97F4A7C15);
    *state = z;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
    z ^ (z >> 31)
}
