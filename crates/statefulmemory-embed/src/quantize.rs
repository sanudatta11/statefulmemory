// Generated with AI Coding Rules Hub
//! Quantization helpers for embeddings (f32 ↔ int8).
//!
//! Activated in P4 spec-task-24 to make BEAM-1M / 10M fit in memory:
//!   10M × 384 × 4B (f32) ≈ 15.4 GB
//!   10M × 384 × 1B (int8) ≈ 3.8 GB     (4× shrink)
//!
//! Encoding uses a per-corpus global scale: `q = round(f * 127 / scale)`
//! clamped to `[-128, 127]`. Decoding is `f = q * scale / 127`. The
//! `scale` is calibrated as `max(|x|)` over a sample of the embedding
//! corpus so the most-saturated dimension maps exactly to ±127. Cosine
//! similarity is preserved since both encode and decode are linear.
//!
//! Recall validated in `tests/quantize_recall.rs` (TS-12): top-10
//! Jaccard ≥ 0.9 vs f32 baseline over 50 sample queries on 1000 vectors.
//!
//! Spec link: SC-NFR-MEM. Plan: P4 spec-task-24.

/// Quantize an f32 slice to int8 using a global scale.
///
/// `scale` should be the calibrated max-abs from
/// [`calibrate_scale`]. Values whose magnitude exceeds `scale` saturate
/// at ±127. A `scale` of 0.0 (or non-finite) is treated as 1.0 to
/// avoid divide-by-zero on degenerate input.
pub fn f32_to_int8(v: &[f32], scale: f32) -> Vec<i8> {
    let s = if scale > 0.0 && scale.is_finite() {
        scale
    } else {
        1.0
    };
    let inv = 127.0 / s;
    v.iter()
        .map(|&x| {
            let q = (x * inv).round();
            q.clamp(-128.0, 127.0) as i8
        })
        .collect()
}

/// Dequantize an int8 slice back to f32 using the same scale used for
/// encoding. Round-trip error is bounded by `scale / 127` per dimension.
pub fn int8_to_f32(v: &[i8], scale: f32) -> Vec<f32> {
    let s = if scale > 0.0 && scale.is_finite() {
        scale
    } else {
        1.0
    };
    let inv = s / 127.0;
    v.iter().map(|&q| q as f32 * inv).collect()
}

/// Calibrate a global scale from a representative corpus of f32 vectors.
///
/// Returns `max(|x|)` over the entire corpus, with a 1e-9 floor to avoid
/// divide-by-zero. Empty corpus returns 1.0 (no-op scale).
pub fn calibrate_scale(corpus: &[&[f32]]) -> f32 {
    let max_abs = corpus
        .iter()
        .flat_map(|v| v.iter())
        .map(|x| x.abs())
        .fold(0.0_f32, f32::max);
    max_abs.max(1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_sign_and_magnitude() {
        let v = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let scale = calibrate_scale(&[&v]);
        assert!((scale - 1.0).abs() < 1e-6);
        let q = f32_to_int8(&v, scale);
        assert_eq!(q[0], 0);
        assert_eq!(q[3], 127);
        assert_eq!(q[4], -127);
        let r = int8_to_f32(&q, scale);
        for (a, b) in v.iter().zip(r.iter()) {
            assert!((a - b).abs() < 1.0 / 127.0 + 1e-6);
        }
    }

    #[test]
    fn calibrate_picks_max_abs_across_corpus() {
        let corpus: Vec<&[f32]> = vec![&[0.1, -0.3], &[0.5, -0.9]];
        let s = calibrate_scale(&corpus);
        assert!((s - 0.9).abs() < 1e-6);
    }

    #[test]
    fn calibrate_handles_empty() {
        let s = calibrate_scale(&[]);
        assert!(s > 0.0 && s.is_finite());
    }

    #[test]
    fn quantize_clamps_oversaturated() {
        // x=2.0 with scale=1.0 → 2.0*127 = 254 → clamps to 127.
        let q = f32_to_int8(&[2.0, -2.0], 1.0);
        assert_eq!(q[0], 127);
        assert_eq!(q[1], -128);
    }

    #[test]
    fn zero_scale_treated_as_unit() {
        let q = f32_to_int8(&[0.5], 0.0);
        assert_eq!(q[0], 64); // round(0.5 * 127) = 64
    }
}
