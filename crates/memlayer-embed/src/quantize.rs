// Generated with AI Coding Rules Hub
//! Quantization helpers for embeddings (f32 ↔ int8).
//!
//! **Status: stubs only.** This module exposes the API surface that
//! `retrieve_hybrid` (spec-task-11) and `facts_writer` (spec-task-17)
//! compile against during P1/P2. The real quantization logic lands in
//! spec-task-24 (P4) where we activate the int8 path for BEAM-1M / 10M.
//!
//! Until then:
//! * [`f32_to_int8`] returns a zero-filled `Vec<i8>` of the right length.
//! * [`int8_to_f32`] returns a zero-filled `Vec<f32>` of the right length.
//! * [`calibrate_scale`] returns `1.0` (a no-op scale).
//!
//! Callers should NOT depend on these doing real work in P1/P2 — they are
//! placeholders that satisfy type-check only. P4 task-24 will replace each
//! body, keeping the signatures stable.
//!
//! Spec links: P4 storage NFR (10M × 384 × 4B → 15GB f32 vs ~3.8GB int8).

/// Quantize an f32 slice to int8 using a global scale.
///
/// **Stub:** returns zero-filled output. P4 spec-task-24 implements
/// `(f * 127.0 / scale).clamp(-128, 127) as i8`.
pub fn f32_to_int8(v: &[f32], _scale: f32) -> Vec<i8> {
    vec![0i8; v.len()]
}

/// Dequantize an int8 slice back to f32 using the same scale used for encoding.
///
/// **Stub:** returns zero-filled output.
pub fn int8_to_f32(v: &[i8], _scale: f32) -> Vec<f32> {
    vec![0.0_f32; v.len()]
}

/// Calibrate a global scale from a representative corpus of f32 vectors.
///
/// **Stub:** always returns `1.0`. P4 spec-task-24 implements
/// `127.0 / corpus.iter().flatten().map(|x| x.abs()).fold(0.0, f32::max).max(1e-9)`.
pub fn calibrate_scale(_corpus: &[&[f32]]) -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_preserve_length() {
        let v = vec![1.0_f32, -2.0, 3.5, -0.25];
        let q = f32_to_int8(&v, 1.0);
        assert_eq!(q.len(), v.len());
        let d = int8_to_f32(&q, 1.0);
        assert_eq!(d.len(), q.len());
    }

    #[test]
    fn calibrate_returns_unit_scale_for_now() {
        let corpus: Vec<&[f32]> = vec![&[1.0, 2.0], &[3.0, 4.0]];
        assert_eq!(calibrate_scale(&corpus), 1.0);
    }
}
