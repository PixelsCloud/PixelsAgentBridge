//! Exact limited-range BT.709 conversion; unsupported layouts retain the scalar path.

pub(super) fn convert(
    input: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
) -> bool {
    if width == 0
        || height == 0
        || width % 4 != 0
        || height % 2 != 0
        || width.checked_mul(4).is_none_or(|row| stride < row)
        || stride
            .checked_mul(height - 1)
            .and_then(|n| n.checked_add(width * 4))
            .is_none_or(|n| input.len() < n)
        || width
            .checked_mul(height)
            .is_none_or(|n| y.len() < n || u.len() < n / 4 || v.len() < n / 4)
    {
        return false;
    }
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    if std::arch::is_x86_feature_detected!("sse4.1") {
        // SAFETY: CPU support and all source/destination row bounds were checked above.
        unsafe {
            sse41(input, stride, width, height, y, u, v);
        }
        return true;
    }
    false
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
#[target_feature(enable = "sse4.1")]
unsafe fn sse41(
    input: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
) {
    #[cfg(target_arch = "x86")]
    use std::arch::x86::*;
    #[cfg(target_arch = "x86_64")]
    use std::arch::x86_64::*;

    // Every load covers four BGRA pixels, never row padding or the next row.
    unsafe {
        let mask = _mm_set1_epi32(255);
        let zero = _mm_setzero_si128();
        let round = _mm_set1_epi32(32768);
        let two = _mm_set1_epi32(2);
        let pack = |n| _mm_cvtsi128_si32(_mm_packus_epi16(_mm_packs_epi32(n, zero), zero)) as u32;
        let dot = |r, g, b, cr, cg, cb, offset| {
            let sum = _mm_add_epi32(
                _mm_add_epi32(
                    _mm_mullo_epi32(r, _mm_set1_epi32(cr)),
                    _mm_mullo_epi32(g, _mm_set1_epi32(cg)),
                ),
                _mm_mullo_epi32(b, _mm_set1_epi32(cb)),
            );
            _mm_add_epi32(
                _mm_srai_epi32::<16>(_mm_add_epi32(sum, round)),
                _mm_set1_epi32(offset),
            )
        };
        for row in (0..height).step_by(2) {
            for col in (0..width).step_by(4) {
                let first = _mm_loadu_si128(input.as_ptr().add(row * stride + col * 4).cast());
                let second =
                    _mm_loadu_si128(input.as_ptr().add((row + 1) * stride + col * 4).cast());
                let (b0, g0, r0) = (
                    _mm_and_si128(first, mask),
                    _mm_and_si128(_mm_srli_epi32::<8>(first), mask),
                    _mm_and_si128(_mm_srli_epi32::<16>(first), mask),
                );
                let (b1, g1, r1) = (
                    _mm_and_si128(second, mask),
                    _mm_and_si128(_mm_srli_epi32::<8>(second), mask),
                    _mm_and_si128(_mm_srli_epi32::<16>(second), mask),
                );
                y[row * width + col..row * width + col + 4]
                    .copy_from_slice(&pack(dot(r0, g0, b0, 11966, 40254, 4064, 16)).to_le_bytes());
                y[(row + 1) * width + col..(row + 1) * width + col + 4]
                    .copy_from_slice(&pack(dot(r1, g1, b1, 11966, 40254, 4064, 16)).to_le_bytes());
                let average = |a, b| {
                    let sum = _mm_add_epi32(a, b);
                    _mm_srli_epi32::<2>(_mm_add_epi32(
                        two,
                        _mm_add_epi32(sum, _mm_shuffle_epi32::<0xb1>(sum)),
                    ))
                };
                let (r, g, b) = (average(r0, r1), average(g0, g1), average(b0, b1));
                let cb = pack(dot(r, g, b, -6596, -22189, 28784, 128)).to_le_bytes();
                let cr = pack(dot(r, g, b, 28784, -26145, -2639, 128)).to_le_bytes();
                let at = row / 2 * (width / 2) + col / 2;
                u[at..at + 2].copy_from_slice(&[cb[0], cb[2]]);
                v[at..at + 2].copy_from_slice(&[cr[0], cr[2]]);
            }
        }
    }
}
