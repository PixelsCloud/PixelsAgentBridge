//! CPU-only conversion regression and timing: rustc -O --test this_file.rs.
#[path = "../crates/desktop-media/src/software_bgra.rs"]
mod bgra;

fn reference(
    input: &[u8],
    stride: usize,
    width: usize,
    height: usize,
    y: &mut [u8],
    u: &mut [u8],
    v: &mut [u8],
) {
    for row in (0..height).step_by(2) {
        for col in (0..width).step_by(2) {
            let mut sum = [0i32; 3];
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = (row + dy) * stride + (col + dx) * 4;
                    let (r, g, b) = (input[p + 2] as i32, input[p + 1] as i32, input[p] as i32);
                    y[(row + dy) * width + col + dx] =
                        (16 + ((11966 * r + 40254 * g + 4064 * b + 32768) >> 16)).clamp(16, 235)
                            as u8;
                    sum[0] += r;
                    sum[1] += g;
                    sum[2] += b;
                }
            }
            let (r, g, b) = ((sum[0] + 2) / 4, (sum[1] + 2) / 4, (sum[2] + 2) / 4);
            let p = row / 2 * (width / 2) + col / 2;
            u[p] = (128 + ((-6596 * r - 22189 * g + 28784 * b + 32768) >> 16)).clamp(16, 240) as u8;
            v[p] = (128 + ((28784 * r - 26145 * g - 2639 * b + 32768) >> 16)).clamp(16, 240) as u8;
        }
    }
}

#[test]
fn accelerated_conversion_matches_bt709_reference_with_unaligned_rows() {
    for (width, height) in [(4, 2), (8, 6), (64, 48), (1920, 1080)] {
        for padding in [0, 1, 19] {
            let stride = width * 4 + padding;
            // No readable padding after the last row; start at an unaligned address.
            let mut data = vec![0; stride * (height - 1) + width * 4 + 1];
            let mut state = 0x12a45678u32;
            for value in &mut data {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                *value = state as u8;
            }
            let input = &data[1..];
            let n = width * height;
            let (mut y, mut u, mut v) = (vec![0; n], vec![0; n / 4], vec![0; n / 4]);
            let (mut ey, mut eu, mut ev) = (y.clone(), u.clone(), v.clone());
            reference(input, stride, width, height, &mut ey, &mut eu, &mut ev);
            if bgra::convert(input, stride, width, height, &mut y, &mut u, &mut v) {
                assert_eq!(y, ey, "luma {width}x{height} padding {padding}");
                assert_eq!(u, eu, "U {width}x{height} padding {padding}");
                assert_eq!(v, ev, "V {width}x{height} padding {padding}");
            } else {
                #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
                assert!(!std::arch::is_x86_feature_detected!("sse4.1"));
            }
        }
    }
}

#[test]
fn accelerated_conversion_preserves_black_white_primary_colors_and_alpha() {
    for rgb in [
        [0, 0, 0],
        [255, 255, 255],
        [255, 0, 0],
        [0, 255, 0],
        [0, 0, 255],
    ] {
        let mut input = Vec::new();
        for alpha in [0, 1, 127, 255, 255, 127, 1, 0] {
            input.extend_from_slice(&[rgb[2], rgb[1], rgb[0], alpha]);
        }
        let (mut y, mut u, mut v) = ([0; 8], [0; 2], [0; 2]);
        let (mut ey, mut eu, mut ev) = (y, u, v);
        reference(&input, 16, 4, 2, &mut ey, &mut eu, &mut ev);
        if bgra::convert(&input, 16, 4, 2, &mut y, &mut u, &mut v) {
            assert_eq!((y, u, v), (ey, eu, ev));
        }
    }
}

#[test]
fn unsupported_or_truncated_buffers_remain_untouched() {
    for (width, height, stride, length) in [
        (0, 2, 16, 32),
        (4, 0, 16, 32),
        (3, 2, 16, 32),
        (4, 3, 16, 48),
        (4, 2, 15, 32),
        (4, 2, 16, 31),
        (usize::MAX - 3, 2, usize::MAX, 32),
        (4, usize::MAX - 1, usize::MAX, 32),
        (8, 4, 32, 128),
    ] {
        let (mut y, mut u, mut v) = ([77; 8], [77; 2], [77; 2]);
        assert!(!bgra::convert(
            &vec![0; length],
            stride,
            width,
            height,
            &mut y,
            &mut u,
            &mut v
        ));
        assert_eq!((y, u, v), ([77; 8], [77; 2], [77; 2]));
    }
}

#[test]
fn report_1080p_conversion_cost_without_a_machine_specific_speed_gate() {
    use std::{hint::black_box, time::Instant};
    let (width, height) = (1920, 1080);
    let frames = if cfg!(debug_assertions) { 2 } else { 40 };
    let mut pixels = vec![0; width * height * 4];
    for (i, value) in pixels.iter_mut().enumerate() {
        *value = (i.wrapping_mul(97) ^ (i >> 9)) as u8;
    }
    let (mut y, mut u, mut v) = (
        vec![0; width * height],
        vec![0; width * height / 4],
        vec![0; width * height / 4],
    );
    if !bgra::convert(&pixels, width * 4, width, height, &mut y, &mut u, &mut v) {
        return;
    }
    let start = Instant::now();
    for _ in 0..frames {
        reference(
            black_box(&pixels),
            width * 4,
            width,
            height,
            &mut y,
            &mut u,
            &mut v,
        );
        black_box((&y, &u, &v));
    }
    let scalar_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
    let start = Instant::now();
    for _ in 0..frames {
        assert!(bgra::convert(
            black_box(&pixels),
            width * 4,
            width,
            height,
            &mut y,
            &mut u,
            &mut v
        ));
        black_box((&y, &u, &v));
    }
    let simd_ms = start.elapsed().as_secs_f64() * 1000.0 / f64::from(frames);
    println!(
        "{{\"width\":{width},\"height\":{height},\"frames\":{frames},\"scalar_ms\":{scalar_ms},\"simd_ms\":{simd_ms},\"display_fps_measured\":false}}"
    );
}
