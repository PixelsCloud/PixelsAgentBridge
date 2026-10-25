use super::*;

#[test]
fn live_bitrate_changes_keep_reference_frames_without_inserting_idr_bursts() {
    let mut encoder = SoftwareEncoder::new(64, 48, 30).unwrap();
    let mut decoder = SoftwareDecoder::new(64, 48).unwrap();
    let pixels = [90u8, 120, 150, 255].repeat(64 * 48);
    let idr = |packet: &[u8]| packet.windows(4).any(|p| p[..3] == [0, 0, 1] && p[3] & 31 == 5);
    encoder.set_encoding_budget(3_000_000).unwrap();
    let first = encoder.encode_bgra(&pixels, 256).unwrap();
    assert!(idr(first));
    assert!(decoder.decode(first).unwrap().is_some());
    for budget in [1_500_000, 750_000, 3_000_000] {
        encoder.set_encoding_budget(budget).unwrap();
        let packet = encoder.encode_bgra(&pixels, 256).unwrap();
        assert!(!idr(packet), "a bitrate-only update restarted the codec at {budget}");
        assert!(decoder.decode(packet).unwrap().is_some());
        let mut actual = openh264_sys2::SBitrateInfo { iLayer: openh264_sys2::SPATIAL_LAYER_ALL, iBitrate: 0 };
        // SAFETY: initialized encoder, correctly sized bitrate option storage.
        let status=unsafe { encoder.encoder.raw_api().get_option(openh264_sys2::ENCODER_OPTION_BITRATE, (&mut actual as *mut openh264_sys2::SBitrateInfo).cast()) };
        assert_eq!(status,0);
        assert_eq!(actual.iBitrate,budget as i32);
    }
    encoder.set_encoding_budget(0).unwrap();
    let restored=encoder.encode_bgra(&pixels,256).unwrap();
    assert!(idr(restored),"changing rate-control mode still requires an independent new stream");
    assert!(SoftwareDecoder::new(64,48).unwrap().decode(restored).unwrap().is_some());
}

#[test]
fn bounded_slice_replacement_preserves_dynamic_color_quality() {
    let (w, h) = (1920usize, 1080usize);
    for bounded in [false, true] {
        let mut encoder = SoftwareEncoder::new(w, h, 30).unwrap();
        if bounded {
            encoder.encoder = SoftwareEncoder::encoder(30, 0, true).unwrap();
            encoder.bounded_slices = true;
        }
        let mut decoder = SoftwareDecoder::new(w, h).unwrap();
        let mut pixels = vec![0u8; w * h * 4];
        let mut maximum = 0.0f64;
        for frame in 205u32..=213 {
            let color = |x: usize, y: usize| -> [u8; 3] {
                if y == 0 || y == 17 {
                    let v = if ((frame >> (x % 16)) & 1) ^ u32::from(x >= 16) == 1 {
                        224
                    } else {
                        32
                    };
                    return [v, v, v];
                }
                [
                    32 + ((frame * 3 + x as u32 * 29 + y as u32 * 17) % 192) as u8,
                    32 + ((frame * 5 + x as u32 * 11 + y as u32 * 31) % 192) as u8,
                    32 + ((frame * 7 + x as u32 * 37 + y as u32 * 13) % 192) as u8,
                ]
            };
            for gy in 0..18 {
                for gx in 0..32 {
                    let rgb = color(gx, gy);
                    let bgra = [rgb[2], rgb[1], rgb[0], 255];
                    for y in gy * h / 18..(gy + 1) * h / 18 {
                        for p in pixels[(y * w + gx * w / 32) * 4..(y * w + (gx + 1) * w / 32) * 4]
                            .chunks_exact_mut(4)
                        {
                            p.copy_from_slice(&bgra);
                        }
                    }
                }
            }
            let packet = encoder.encode_bgra(&pixels, w * 4).unwrap();
            let output = decoder.decode(packet).unwrap().unwrap();
            for gy in 0..18 {
                for gx in 0..32 {
                    let x = w * (2 * gx + 1) / 64;
                    let y = h * (2 * gy + 1) / 36;
                    let uv = w * h + (y / 2) * w + (x / 2) * 2;
                    let (l, u, v) = (
                        1.16438356 * (f64::from(output[y * w + x]) - 16.),
                        f64::from(output[uv]) - 128.,
                        f64::from(output[uv + 1]) - 128.,
                    );
                    let rgb = [
                        l + 1.79274107 * v,
                        l - 0.21324861 * u - 0.53290933 * v,
                        l + 2.11240179 * u,
                    ];
                    for (a, b) in rgb.into_iter().zip(color(gx, gy)) {
                        maximum = maximum.max((a - f64::from(b)).abs());
                    }
                }
            }
        }
        eprintln!("bounded_slice_color bounded={bounded} max_rgb_error={maximum}");
        assert!(
            maximum <= 16.,
            "bounded={bounded} changed color fidelity: {maximum}"
        );
    }
}
