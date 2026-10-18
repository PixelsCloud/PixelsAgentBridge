//! Portable, bundled H.264 fallback. Used only when direct GPU codecs cannot
//! operate; its readback/conversion cost must never be attributed to hardware.
use openh264::{
    OpenH264API,
    decoder::Decoder,
    encoder::{
        Complexity, Encoder, EncoderConfig, FrameRate, Profile, QpRange, RateControlMode,
        UsageType, VuiConfig,
    },
    formats::{YUVSlices, YUVSource},
};
type Error = Box<dyn std::error::Error + Send + Sync>;
// Keep fine chroma residuals after desktop transitions. QP 20 still left
// persistent color errors with different multi-slice boundaries.
const SOFTWARE_QP: u8 = 14;

fn dimensions(width: usize, height: usize) -> Result<(usize, usize), Error> {
    if width == 0
        || height == 0
        || width > 8192
        || height > 8192
        || (width + 1) / 2 * 2 * ((height + 1) / 2 * 2) > 33_554_432
    {
        return Err("software H.264 dimensions exceed bounds".into());
    }
    Ok(((width + 1) & !1, (height + 1) & !1))
}

struct Planar {
    visible_width: usize,
    visible_height: usize,
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}
impl Planar {
    fn new(width: usize, height: usize) -> Result<Self, Error> {
        let (coded_width, coded_height) = dimensions(width, height)?;
        let pixels = coded_width * coded_height;
        Ok(Self {
            visible_width: width,
            visible_height: height,
            width: coded_width,
            height: coded_height,
            y: vec![0; pixels],
            u: vec![0; pixels / 4],
            v: vec![0; pixels / 4],
        })
    }
    fn bgra(&mut self, input: &[u8], stride: usize) -> Result<(), Error> {
        let minimum = stride
            .checked_mul(self.visible_height - 1)
            .and_then(|n| n.checked_add(self.visible_width * 4))
            .ok_or("BGRA stride overflow")?;
        if stride < self.visible_width * 4 || input.len() < minimum {
            return Err("BGRA buffer is truncated".into());
        }
        // Preserve the even-size hot path. Odd images replicate edge pixels
        // while converting, without allocating/copying an extra BGRA image.
        if self.width == self.visible_width && self.height == self.visible_height {
            self.convert::<false>(input, stride);
        } else {
            self.convert::<true>(input, stride);
        }
        Ok(())
    }
    fn convert<const PAD: bool>(&mut self, input: &[u8], stride: usize) {
        // Limited-range BT.709, integer coefficients scaled by 65536. Chroma
        // averages the 2x2 source block, including all rows and their padding.
        // The dependency's convenience RGB conversion uses a different matrix.
        for row in (0..self.height).step_by(2) {
            for col in (0..self.width).step_by(2) {
                let mut rgb = [0i32; 3];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let (source_y, source_x) = if PAD {
                            (
                                (row + dy).min(self.visible_height - 1),
                                (col + dx).min(self.visible_width - 1),
                            )
                        } else {
                            (row + dy, col + dx)
                        };
                        let p = source_y * stride + source_x * 4;
                        let (r, g, b) = (
                            i32::from(input[p + 2]),
                            i32::from(input[p + 1]),
                            i32::from(input[p]),
                        );
                        self.y[(row + dy) * self.width + col + dx] = (16
                            + ((11966 * r + 40254 * g + 4064 * b + 32768) >> 16))
                            .clamp(16, 235)
                            as u8;
                        rgb[0] += r;
                        rgb[1] += g;
                        rgb[2] += b;
                    }
                }
                let (r, g, b) = ((rgb[0] + 2) / 4, (rgb[1] + 2) / 4, (rgb[2] + 2) / 4);
                let p = row / 2 * (self.width / 2) + col / 2;
                self.u[p] = (128 + ((-6596 * r - 22189 * g + 28784 * b + 32768) >> 16))
                    .clamp(16, 240) as u8;
                self.v[p] =
                    (128 + ((28784 * r - 26145 * g - 2639 * b + 32768) >> 16)).clamp(16, 240) as u8;
            }
        }
    }
    fn nv12<'a>(
        &'a mut self,
        y: &'a [u8],
        y_stride: usize,
        uv: &[u8],
        uv_stride: usize,
    ) -> Result<YUVSlices<'a>, Error> {
        let y_len = y_stride
            .checked_mul(self.visible_height)
            .ok_or("NV12 luma stride overflow")?;
        let uv_len = uv_stride
            .checked_mul(self.height / 2)
            .ok_or("NV12 chroma stride overflow")?;
        if y_stride < self.visible_width
            || uv_stride < self.width
            || y.len() < y_len
            || uv.len() < uv_len
        {
            return Err("NV12 planes are truncated".into());
        }
        // OpenH264 accepts I420. Borrow the captured luma for even geometry;
        // only separate interleaved chroma, without an NV12 -> RGB -> YUV trip.
        for row in 0..self.height / 2 {
            for col in 0..self.width / 2 {
                let source = row * uv_stride + col * 2;
                let target = row * (self.width / 2) + col;
                self.u[target] = uv[source];
                self.v[target] = uv[source + 1];
            }
        }
        let (luma, stride) =
            if self.width == self.visible_width && self.height == self.visible_height {
                (&y[..y_len], y_stride)
            } else {
                for row in 0..self.height {
                    let source = row.min(self.visible_height - 1) * y_stride;
                    let target = &mut self.y[row * self.width..(row + 1) * self.width];
                    target[..self.visible_width]
                        .copy_from_slice(&y[source..source + self.visible_width]);
                    if self.width != self.visible_width {
                        target[self.visible_width] = target[self.visible_width - 1];
                    }
                }
                (self.y.as_slice(), self.width)
            };
        Ok(YUVSlices::new(
            (luma, &self.u, &self.v),
            (self.width, self.height),
            (stride, self.width / 2, self.width / 2),
        ))
    }
}

pub struct SoftwareEncoder {
    encoder: Encoder,
    planar: Planar,
    packet: Vec<u8>,
    initialized: bool,
    prepared_bgra: bool,
}
impl SoftwareEncoder {
    pub fn new(width: usize, height: usize, fps: u16) -> Result<Self, Error> {
        let planar = Planar::new(width, height)?;
        if !(1..=60).contains(&fps) {
            return Err("invalid software encoder frame rate".into());
        }
        // Fixed quality keeps every changed frame and avoids hidden rate-control
        // drops. Transport credit provides backpressure; bitrate is measured.
        let config = EncoderConfig::new()
            .num_threads(4)
            .skip_frames(false)
            .max_frame_rate(FrameRate::from_hz(f32::from(fps)))
            .usage_type(UsageType::ScreenContentRealTime)
            .complexity(Complexity::Low)
            .profile(Profile::Baseline)
            .rate_control_mode(RateControlMode::Off)
            .qp(QpRange::new(SOFTWARE_QP, SOFTWARE_QP))
            .adaptive_quantization(false)
            .background_detection(false)
            .vui(VuiConfig::bt709());
        Ok(Self {
            encoder: Encoder::with_api_config(OpenH264API::from_source(), config)?,
            planar,
            packet: Vec::new(),
            initialized: false,
            prepared_bgra: false,
        })
    }
    /// The returned Annex-B access unit stays valid until the next encode.
    pub fn encode_bgra(&mut self, input: &[u8], stride: usize) -> Result<&[u8], Error> {
        self.prepare_bgra(input, stride)?;
        self.encode_prepared_bgra()
    }
    /// Convert into the existing owned planes. No source pointer survives this
    /// call, so a capture pool buffer can be returned before codec work starts.
    pub(crate) fn prepare_bgra(&mut self, input: &[u8], stride: usize) -> Result<(), Error> {
        self.prepared_bgra = false;
        self.planar.bgra(input, stride)?;
        self.prepared_bgra = true;
        Ok(())
    }
    pub(crate) fn encode_prepared_bgra(&mut self) -> Result<&[u8], Error> {
        if !std::mem::take(&mut self.prepared_bgra) {
            return Err("software frame has not been prepared".into());
        }
        let p = &self.planar;
        let source = YUVSlices::new(
            (&p.y, &p.u, &p.v),
            (p.width, p.height),
            (p.width, p.width / 2, p.width / 2),
        );
        Self::encode_frame(
            &mut self.encoder,
            &mut self.packet,
            &mut self.initialized,
            &source,
        )
    }
    /// Limited-range BT.709 NV12 from a mapped capture surface. The hardware
    /// path never invokes this method. Both strides include full final rows.
    pub fn encode_nv12(
        &mut self,
        y: &[u8],
        y_stride: usize,
        uv: &[u8],
        uv_stride: usize,
    ) -> Result<&[u8], Error> {
        self.prepared_bgra = false;
        let source = self.planar.nv12(y, y_stride, uv, uv_stride)?;
        Self::encode_frame(
            &mut self.encoder,
            &mut self.packet,
            &mut self.initialized,
            &source,
        )
    }
    fn encode_frame<'a>(
        encoder: &mut Encoder,
        packet: &'a mut Vec<u8>,
        initialized: &mut bool,
        source: &YUVSlices<'_>,
    ) -> Result<&'a [u8], Error> {
        if !*initialized {
            // The wrapper initializes dimensions lazily, and forces a single
            // slice even with num_threads(4). Prime it privately, then enable
            // independent slices; the first published frame must be a new IDR.
            encoder.encode(source)?;
            {
                use openh264_sys2::{
                    ENCODER_OPTION_SVC_ENCODE_PARAM_EXT, SEncParamExt, SM_FIXEDSLCNUM_SLICE,
                };
                let mut params = std::mem::MaybeUninit::<SEncParamExt>::zeroed();
                // SAFETY: initialized encoder, correctly sized C parameter
                // storage, unchanged dimensions/color/layers and buffer API.
                // RC_OFF_MODE reads iDLayerQp, not iMinQp/iMaxQp. Apply the
                // intended quality after lazy initialization, including small
                // single-slice frames; otherwise the upstream default wins.
                unsafe {
                    let api = encoder.raw_api();
                    if api.get_option(
                        ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        params.as_mut_ptr().cast(),
                    ) != 0
                    {
                        return Err("software encoder parameters unavailable".into());
                    }
                    let mut params = params.assume_init();
                    params.sSpatialLayers[0].iDLayerQp = i32::from(SOFTWARE_QP);
                    let (width, height) = source.dimensions();
                    if width * height >= 65_536 {
                        params.iMultipleThreadIdc = 4;
                        params.sSpatialLayers[0].sSliceArgument.uiSliceMode = SM_FIXEDSLCNUM_SLICE;
                        params.sSpatialLayers[0].sSliceArgument.uiSliceNum = 4;
                    }
                    if api.set_option(
                        ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        (&mut params as *mut SEncParamExt).cast(),
                    ) != 0
                    {
                        return Err("software encoder configuration failed".into());
                    }
                }
            }
            encoder.force_intra_frame();
            *initialized = true;
        }
        packet.clear();
        encoder.encode(source)?.write_vec(packet);
        if packet.is_empty() || packet.len() > pab_protocol::DESKTOP_MAX_FRAME_BYTES {
            return Err("software encoder skipped a frame or exceeded the packet bound".into());
        }
        Ok(packet)
    }
}

pub struct SoftwareDecoder {
    decoder: Decoder,
    width: usize,
    height: usize,
    nv12: Vec<u8>,
}
impl SoftwareDecoder {
    pub fn new(width: usize, height: usize) -> Result<Self, Error> {
        let (width, height) = dimensions(width, height)?;
        let pixels = width * height;
        Ok(Self {
            decoder: Decoder::new()?,
            width,
            height,
            nv12: vec![0; pixels * 3 / 2],
        })
    }
    /// CPU NV12, tightly packed at the dimensions rounded up to even, limited
    /// BT.709. The presenter crops to the session's original visible dimensions.
    /// None means no decoded output yet; it must never grant presentation credit.
    pub fn decode(&mut self, packet: &[u8]) -> Result<Option<&[u8]>, Error> {
        if packet.is_empty() || packet.len() > pab_protocol::DESKTOP_MAX_FRAME_BYTES {
            return Err("invalid software decoder packet length".into());
        }
        let Some(frame) = self.decoder.decode(packet)? else {
            return Ok(None);
        };
        if frame.dimensions() != (self.width, self.height) {
            return Err("software decoder dimensions changed".into());
        }
        let (sy, su, sv) = frame.strides();
        self.nv12.resize(self.width * self.height * 3 / 2, 0);
        for row in 0..self.height {
            self.nv12[row * self.width..(row + 1) * self.width]
                .copy_from_slice(&frame.y()[row * sy..row * sy + self.width]);
        }
        let offset = self.width * self.height;
        for row in 0..self.height / 2 {
            for col in 0..self.width / 2 {
                let p = offset + row * self.width + col * 2;
                self.nv12[p] = frame.u()[row * su + col];
                self.nv12[p + 1] = frame.v()[row * sv + col];
            }
        }
        Ok(Some(&self.nv12))
    }

    /// Transfer the output allocation to a UI thread and recycle it after GPU
    /// upload. Moving the Vec does not copy its pixels or borrow the decoder.
    #[cfg(target_os = "linux")]
    pub(crate) fn decode_owned(
        &mut self,
        packet: &[u8],
        recycled: Vec<u8>,
    ) -> Result<Option<Vec<u8>>, Error> {
        if !recycled.is_empty() {
            self.nv12 = recycled;
        }
        if self.decode(packet)?.is_some() {
            Ok(Some(std::mem::take(&mut self.nv12)))
        } else {
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prepared_bgra_owns_converted_pixels_after_capture_buffer_is_reused() {
        let (width, height) = (63, 47);
        let mut direct = SoftwareEncoder::new(width, height, 15).unwrap();
        let mut split = SoftwareEncoder::new(width, height, 15).unwrap();
        for index in 0..8 {
            let mut pixels = [31 + index * 7, 190 - index * 9, 55 + index * 13, 255]
                .repeat(width * height);
            let expected = direct.encode_bgra(&pixels, width * 4).unwrap().to_vec();
            split.prepare_bgra(&pixels, width * 4).unwrap();
            pixels.fill(0xff);
            drop(pixels);
            assert_eq!(split.encode_prepared_bgra().unwrap(), expected);
            assert!(split.encode_prepared_bgra().is_err());
        }
    }
    #[test]
    fn failed_preparation_or_nv12_switch_cannot_encode_old_bgra() {
        let mut encoder = SoftwareEncoder::new(64, 48, 15).unwrap();
        let pixels = vec![127; 64 * 48 * 4];
        assert!(encoder.encode_prepared_bgra().is_err());
        encoder.prepare_bgra(&pixels, 256).unwrap();
        assert!(encoder.prepare_bgra(&pixels[..31], 256).is_err());
        assert!(encoder.encode_prepared_bgra().is_err());
        encoder.prepare_bgra(&pixels, 256).unwrap();
        assert!(encoder.encode_nv12(&[], 64, &[], 64).is_err());
        assert!(encoder.encode_prepared_bgra().is_err());
        assert!(!encoder.encode_bgra(&pixels, 256).unwrap().is_empty());
    }
    /// Diagnostic only: generated desktop bars isolate conversion and codec
    /// cost from compositor, capture, transport and presentation scheduling.
    #[test]
    #[ignore = "manual timing on the target hardware; not a display FPS acceptance test"]
    fn profile_1080p_software_pipeline() {
        use std::time::Instant;
        let (width, height) = (1920, 1080);
        let mut encoder = SoftwareEncoder::new(width, height, 15).unwrap();
        let mut decoder = SoftwareDecoder::new(width, height).unwrap();
        let mut pixels = vec![0u8; width * height * 4];
        let mut samples = Vec::new();
        for index in 0..90 {
            for y in 0..height {
                for x in 0..width {
                    let phase = (index * 3 + (x / 120) * 10 + (y / 90) * 10) % 128;
                    let p = (y * width + x) * 4;
                    pixels[p..p + 4].copy_from_slice(&[
                        89, ((128 - phase) * 255 / 128) as u8,
                        (phase * 255 / 128) as u8, 255,
                    ]);
                }
            }
            let started = Instant::now();
            encoder.planar.bgra(&pixels, width * 4).unwrap();
            let converted = Instant::now();
            let p = &encoder.planar;
            let source = YUVSlices::new(
                (&p.y, &p.u, &p.v), (p.width, p.height),
                (p.width, p.width / 2, p.width / 2),
            );
            let packet = SoftwareEncoder::encode_frame(
                &mut encoder.encoder, &mut encoder.packet, &mut encoder.initialized, &source,
            ).unwrap();
            let encoded = Instant::now();
            let bytes = packet.len();
            assert!(decoder.decode(packet).unwrap().is_some());
            let decoded = Instant::now();
            if index >= 15 {
                samples.push(serde_json::json!({
                    "conversion_ms": (converted-started).as_secs_f64()*1000.0,
                    "encode_ms": (encoded-converted).as_secs_f64()*1000.0,
                    "decode_ms": (decoded-encoded).as_secs_f64()*1000.0,
                    "bytes": bytes,
                }));
            }
        }
        println!("{}", serde_json::json!({
            "dimensions": [width, height], "warmup_frames": 15,
            "sample_frames": samples.len(), "source": "generated changing bars",
            "display_fps_measured": false, "samples": samples,
        }));
    }
    #[test]
    fn nv12_borrows_even_luma_and_preserves_odd_edges_with_padded_strides() {
        for (width, height) in [(64, 48), (63, 48), (64, 47), (63, 47)] {
            let mut planar = Planar::new(width, height).unwrap();
            let (cw, ch) = dimensions(width, height).unwrap();
            let (ys, uvs) = (width + 13, cw + 10);
            let mut y = vec![0xff; ys * height];
            let mut uv = vec![0xff; uvs * (ch / 2)];
            for row in 0..height {
                for col in 0..width {
                    y[row * ys + col] = (row + col) as u8;
                }
            }
            for row in 0..ch / 2 {
                for col in 0..cw / 2 {
                    uv[row * uvs + col * 2] = (31 + row) as u8;
                    uv[row * uvs + col * 2 + 1] = (97 + col) as u8;
                }
            }
            let source = planar.nv12(&y, ys, &uv, uvs).unwrap();
            if cw == width && ch == height {
                assert_eq!(source.y().as_ptr(), y.as_ptr());
            }
            for row in 0..ch {
                for col in 0..cw {
                    assert_eq!(
                        source.y()[row * source.strides().0 + col],
                        (row.min(height - 1) + col.min(width - 1)) as u8
                    );
                }
            }
            for row in 0..ch / 2 {
                for col in 0..cw / 2 {
                    assert_eq!(source.u()[row * cw / 2 + col], (31 + row) as u8);
                    assert_eq!(source.v()[row * cw / 2 + col], (97 + col) as u8);
                }
            }
        }
    }
    #[test]
    fn nv12_rejects_invalid_extents_before_reading() {
        let mut p = Planar::new(63, 47).unwrap();
        let y = vec![16; 64 * 47];
        let uv = vec![128; 64 * 24];
        assert!(p.nv12(&y, usize::MAX, &uv, 64).is_err());
        assert!(p.nv12(&y, 64, &uv, usize::MAX).is_err());
        assert!(p.nv12(&y, 62, &uv, 64).is_err());
        assert!(p.nv12(&y, 64, &uv, 63).is_err());
        assert!(p.nv12(&y[..y.len() - 1], 64, &uv, 64).is_err());
        assert!(p.nv12(&y, 64, &uv[..uv.len() - 1], 64).is_err());
        assert!(p.nv12(&y, 64, &uv, 64).is_ok());
    }
    #[test]
    fn nv12_dynamic_encode_decode_preserves_limited_bt709_samples() {
        for (width, height) in [(64, 48), (257, 129)] {
            let (cw, ch) = dimensions(width, height).unwrap();
            let mut encoder = SoftwareEncoder::new(width, height, 15).unwrap();
            let mut decoder = SoftwareDecoder::new(width, height).unwrap();
            let (ys, uvs) = (width + 16, cw + 16);
            let mut y = vec![0; ys * height];
            let mut uv = vec![0; uvs * ch / 2];
            for index in 0..8u8 {
                let expected = [30 + index * 17, 50 + index * 11, 210 - index * 13];
                for row in 0..height {
                    y[row * ys..row * ys + width].fill(expected[0]);
                }
                for row in 0..ch / 2 {
                    for col in 0..cw / 2 {
                        uv[row * uvs + col * 2] = expected[1];
                        uv[row * uvs + col * 2 + 1] = expected[2];
                    }
                }
                let packet = encoder.encode_nv12(&y, ys, &uv, uvs).unwrap();
                let output = decoder.decode(packet).unwrap().expect("no delayed frame");
                for (x, row) in [(0, 0), (cw / 2, ch / 2), (cw - 1, ch - 1)] {
                    let chroma = cw * ch + (row / 2) * cw + (x / 2) * 2;
                    for (actual, expected) in
                        [output[row * cw + x], output[chroma], output[chroma + 1]]
                            .into_iter()
                            .zip(expected)
                    {
                        assert!(actual.abs_diff(expected) <= 3, "{actual} versus {expected}");
                    }
                }
            }
        }
    }
    #[test]
    fn settled_color_is_stable_after_spatial_fade() {
        // A real GNOME transition left this generated background with a
        // persistent chroma shift. Exercise both single- and multi-slice paths.
        // Check the original RGB error budget, not lossless YUV equality: a
        // one-step chroma change does not map to one RGB step in BT.709.
        for (width, height) in [(64, 48), (512, 288)] {
            let mut encoder = SoftwareEncoder::new(width, height, 15).unwrap();
            let mut decoder = SoftwareDecoder::new(width, height).unwrap();
            let mut bgra = vec![0; width * height * 4];
            for index in 0..24 {
                for (i, pixel) in bgra.chunks_exact_mut(4).enumerate() {
                    let (col, row) = (i % width / 8, i / width / 8);
                    let pattern = [
                        ((col * 7 + row * 11) & 4) / 4 * 255,
                        ((col * 3 ^ row * 13) & 2) / 2 * 255,
                        ((col * 17 + row * 31) & 1) * 255,
                    ];
                    for (channel, target) in [74, 81, 20].into_iter().enumerate() {
                        let phase = index.min(20);
                        pixel[channel] =
                            ((pattern[channel] * (20 - phase) + target * phase) / 20) as u8;
                    }
                    pixel[3] = 255;
                }
                let packet = encoder.encode_bgra(&bgra, width * 4).unwrap();
                let decoded = decoder.decode(packet).unwrap().unwrap();
                if index >= 20 {
                    for y in [height / 3, height / 2, height * 3 / 4] {
                        for x in [width / 4, width / 2, width * 3 / 4] {
                            let uv = width * height + y / 2 * width + x / 2 * 2;
                            let luma = (f64::from(decoded[y * width + x]) - 16.) * 255. / 219.;
                            let cb = (f64::from(decoded[uv]) - 128.) * 255. / 224.;
                            let cr = (f64::from(decoded[uv + 1]) - 128.) * 255. / 224.;
                            let rgb = [
                                luma + 1.5748 * cr,
                                luma - 0.187324 * cb - 0.468124 * cr,
                                luma + 1.8556 * cb,
                            ];
                            for (actual, wanted) in rgb.into_iter().zip([20., 81., 74.]) {
                                assert!(
                                    (actual - wanted).abs() <= 2.,
                                    "settled color at {width}x{height}, frame {index}, ({x},{y}): {rgb:?}"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn owned_decode_reuses_allocation_and_matches_borrowed_pixels() {
        let fixture = include_bytes!("../tests/fixtures/apple-low-latency.pabvideo");
        let number = |offset: usize| {
            u32::from_le_bytes(fixture[offset..offset + 4].try_into().unwrap()) as usize
        };
        let mut owned = SoftwareDecoder::new(number(8), number(12)).unwrap();
        let mut borrowed = SoftwareDecoder::new(number(8), number(12)).unwrap();
        let mut recycled = Vec::new();
        let mut allocation: *const u8 = std::ptr::null();
        let mut offset = 24;
        for _ in 0..number(20) {
            let size = number(offset);
            offset += 24;
            let packet = &fixture[offset..offset + size];
            offset += size;
            recycled = owned.decode_owned(packet, recycled).unwrap().unwrap();
            assert_eq!(
                recycled.as_slice(),
                borrowed.decode(packet).unwrap().unwrap()
            );
            if allocation.is_null() {
                allocation = recycled.as_ptr();
            }
            assert_eq!(
                recycled.as_ptr(),
                allocation,
                "NV12 storage was unnecessarily reallocated"
            );
        }
        assert_eq!(offset, fixture.len());
    }
    #[test]
    fn apple_low_latency_stream_has_immediate_ordered_pixels() {
        // Real M4 VideoToolbox output, generated quadrants only. This catches
        // stale images that a self-encoded OpenH264 round trip cannot expose.
        let fixture = include_bytes!("../tests/fixtures/apple-low-latency.pabvideo");
        assert_eq!(&fixture[..8], b"PABMAC1\0");
        let number =
            |offset: usize| u32::from_le_bytes(fixture[offset..offset + 4].try_into().unwrap());
        let (width, height) = (number(8) as usize, number(12) as usize);
        let mut decoder = SoftwareDecoder::new(width, height).unwrap();
        let mut offset = 24;
        for index in 0..number(20) as usize {
            let size = number(offset) as usize;
            assert_eq!(
                u64::from_le_bytes(fixture[offset + 4..offset + 12].try_into().unwrap()),
                index as u64
            );
            offset += 24;
            let frame = decoder
                .decode(&fixture[offset..offset + size])
                .unwrap()
                .expect("each low-latency frame must be immediately available");
            offset += size;
            for quadrant in 0..4 {
                let rgb = [
                    24 + (index * 3 + quadrant * 43) % 208,
                    24 + (index * 7 + quadrant * 67) % 208,
                    24 + (index * 11 + quadrant * 97) % 208,
                ];
                let mut expected = Planar::new(2, 2).unwrap();
                expected
                    .bgra(
                        &[rgb[2] as u8, rgb[1] as u8, rgb[0] as u8, 255].repeat(4),
                        8,
                    )
                    .unwrap();
                let x = (quadrant % 2) * width / 2 + width / 4;
                let y = (quadrant / 2) * height / 2 + height / 4;
                let uv = width * height + (y / 2) * width + (x / 2) * 2;
                for (actual, wanted) in [
                    (frame[y * width + x], expected.y[0]),
                    (frame[uv], expected.u[0]),
                    (frame[uv + 1], expected.v[0]),
                ] {
                    assert!(
                        actual.abs_diff(wanted) <= 5,
                        "stale/color error at frame {index}, quadrant {quadrant}"
                    );
                }
            }
        }
        assert_eq!(offset, fixture.len());
    }

    #[test]
    fn bt709_primary_colors_padding_and_truncation() {
        for (rgb, expected) in [
            ([0, 0, 0], [16, 128, 128]),
            ([255, 255, 255], [235, 128, 128]),
            ([255, 0, 0], [63, 102, 240]),
            ([0, 255, 0], [173, 42, 26]),
            ([0, 0, 255], [32, 240, 118]),
        ] {
            let mut p = Planar::new(2, 2).unwrap();
            let mut padded = vec![77; 24];
            for row in 0..2 {
                for col in 0..2 {
                    padded[row * 12 + col * 4..row * 12 + col * 4 + 4]
                        .copy_from_slice(&[rgb[2], rgb[1], rgb[0], 255]);
                }
            }
            p.bgra(&padded, 12).unwrap();
            assert_eq!(p.y, vec![expected[0]; 4]);
            assert_eq!(p.u, [expected[1]]);
            assert_eq!(p.v, [expected[2]]);
            assert!(p.bgra(&padded[..19], 12).is_err());
            assert!(p.bgra(&padded, 7).is_err());
            assert!(p.bgra(&padded, usize::MAX).is_err());
        }
        assert!(SoftwareEncoder::new(0, 48, 15).is_err());
        assert!(SoftwareDecoder::new(8192, 8192).is_err());
    }
    #[test]
    fn odd_source_edges_are_replicated_without_reading_stride_padding() {
        let mut p = Planar::new(3, 3).unwrap();
        let stride = 20;
        // The last row deliberately omits padding; every unused byte is poison.
        let mut bgra = vec![77; stride * 2 + 12];
        for y in 0..3 {
            for x in 0..3 {
                let value = if x == 2 || y == 2 { 255 } else { 0 };
                bgra[y * stride + x * 4..y * stride + x * 4 + 4]
                    .copy_from_slice(&[value, value, value, 255]);
            }
        }
        p.bgra(&bgra, stride).unwrap();
        assert_eq!((p.width, p.height), (4, 4));
        assert_eq!(
            p.y,
            [
                16, 16, 235, 235, 16, 16, 235, 235, 235, 235, 235, 235, 235, 235, 235, 235
            ]
        );
        assert_eq!(p.u, [128; 4]);
        assert_eq!(p.v, [128; 4]);
        assert!(p.bgra(&bgra[..bgra.len() - 1], stride).is_err());
        assert!(dimensions(usize::MAX, 1).is_err());
        assert_eq!(dimensions(1, 1).unwrap(), (2, 2));
        assert_eq!(dimensions(8191, 4095).unwrap(), (8192, 4096));
        assert!(dimensions(8191, 4097).is_err());
    }
    #[test]
    fn odd_h264_retains_last_visible_row_and_column() {
        for (width, height) in [(63, 48), (64, 47), (1297, 753)] {
            let mut encoder = SoftwareEncoder::new(width, height, 15).unwrap();
            let mut decoder = SoftwareDecoder::new(width, height).unwrap();
            let stride = width * 4 + 12;
            let mut source = vec![77; (height - 1) * stride + width * 4];
            for y in 0..height {
                for x in 0..width {
                    let value = if x == width - 1 || y == height - 1 {
                        255
                    } else {
                        0
                    };
                    source[y * stride + x * 4..y * stride + x * 4 + 4]
                        .copy_from_slice(&[value, value, value, 255]);
                }
            }
            let (cw, ch) = dimensions(width, height).unwrap();
            for _ in 0..3 {
                let packet = encoder.encode_bgra(&source, stride).unwrap();
                let image = decoder.decode(packet).unwrap().unwrap();
                assert_eq!(image.len(), cw * ch * 3 / 2);
                for y in 0..height {
                    assert!(image[y * cw + width - 1] > 210, "last column lost");
                }
                for x in 0..width {
                    assert!(image[(height - 1) * cw + x] > 210, "last row lost");
                }
                assert!(
                    image[(height / 2) * cw + width / 2] < 40,
                    "image scaled or shifted"
                );
            }
        }
    }
    #[test]
    fn changing_frames_round_trip_without_skips_or_stale_pixels() {
        let mut encoder = SoftwareEncoder::new(64, 48, 30).unwrap();
        let mut decoder = SoftwareDecoder::new(64, 48).unwrap();
        let mut bgra = vec![0; 64 * 48 * 4];
        let mut reference = Planar::new(64, 48).unwrap();
        for index in 0..12 {
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[20 + index * 12, 60 + index * 5, 220 - index * 8, 255]);
            }
            reference.bgra(&bgra, 256).unwrap();
            let packet = encoder.encode_bgra(&bgra, 256).unwrap();
            let nv12 = decoder
                .decode(packet)
                .unwrap()
                .expect("a complete low-latency access unit must produce a frame");
            assert_eq!(nv12.len(), 64 * 48 * 3 / 2);
            assert!(
                (i16::from(nv12[24 * 64 + 32]) - i16::from(reference.y[24 * 64 + 32])).abs() <= 5
            );
            let p = 64 * 48 + 12 * 64 + 32;
            assert!((i16::from(nv12[p]) - i16::from(reference.u[12 * 32 + 16])).abs() <= 5);
            assert!((i16::from(nv12[p + 1]) - i16::from(reference.v[12 * 32 + 16])).abs() <= 5);
        }
    }

    #[test]
    fn multi_slice_stream_starts_with_independently_decodable_frame() {
        let mut encoder = SoftwareEncoder::new(512, 288, 15).unwrap();
        let mut decoder = SoftwareDecoder::new(512, 288).unwrap();
        let mut bgra = vec![0; 512 * 288 * 4];
        for index in 0..5u8 {
            for pixel in bgra.chunks_exact_mut(4) {
                pixel.copy_from_slice(&[index * 40, 80, 170, 255]);
            }
            let packet = encoder.encode_bgra(&bgra, 512 * 4).unwrap();
            if index == 0 {
                assert!(openh264::nal_units(packet).any(|nal| {
                    let offset = if nal.starts_with(&[0, 0, 0, 1]) { 4 } else { 3 };
                    nal.get(offset).is_some_and(|value| value & 31 == 5)
                }));
            }
            assert_eq!(
                decoder.decode(packet).unwrap().unwrap().len(),
                512 * 288 * 3 / 2
            );
        }
    }
}
