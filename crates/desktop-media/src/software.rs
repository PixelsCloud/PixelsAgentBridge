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

fn dimensions(width: usize, height: usize) -> Result<usize, Error> {
    if width == 0
        || height == 0
        || width % 2 != 0
        || height % 2 != 0
        || width > 8192
        || height > 8192
        || width * height > 33_554_432
    {
        return Err("software H.264 requires bounded even dimensions".into());
    }
    Ok(width * height)
}

struct Planar {
    width: usize,
    height: usize,
    y: Vec<u8>,
    u: Vec<u8>,
    v: Vec<u8>,
}
impl Planar {
    fn new(width: usize, height: usize) -> Result<Self, Error> {
        let pixels = dimensions(width, height)?;
        Ok(Self {
            width,
            height,
            y: vec![0; pixels],
            u: vec![0; pixels / 4],
            v: vec![0; pixels / 4],
        })
    }
    fn bgra(&mut self, input: &[u8], stride: usize) -> Result<(), Error> {
        let minimum = stride
            .checked_mul(self.height - 1)
            .and_then(|n| n.checked_add(self.width * 4))
            .ok_or("BGRA stride overflow")?;
        if stride < self.width * 4 || input.len() < minimum {
            return Err("BGRA buffer is truncated".into());
        }
        // Limited-range BT.709, integer coefficients scaled by 65536. Chroma
        // averages the 2x2 source block, including all rows and their padding.
        // The dependency's convenience RGB conversion uses a different matrix.
        for row in (0..self.height).step_by(2) {
            for col in (0..self.width).step_by(2) {
                let mut rgb = [0i32; 3];
                for dy in 0..2 {
                    for dx in 0..2 {
                        let p = (row + dy) * stride + (col + dx) * 4;
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
        Ok(())
    }
}

pub struct SoftwareEncoder {
    encoder: Encoder,
    planar: Planar,
    packet: Vec<u8>,
    initialized: bool,
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
            .qp(QpRange::new(20, 28))
            .adaptive_quantization(false)
            .background_detection(false)
            .vui(VuiConfig::bt709());
        Ok(Self {
            encoder: Encoder::with_api_config(OpenH264API::from_source(), config)?,
            planar,
            packet: Vec::new(),
            initialized: false,
        })
    }
    /// The returned Annex-B access unit stays valid until the next encode.
    pub fn encode_bgra(&mut self, input: &[u8], stride: usize) -> Result<&[u8], Error> {
        self.planar.bgra(input, stride)?;
        let p = &self.planar;
        let source = YUVSlices::new(
            (&p.y, &p.u, &p.v),
            (p.width, p.height),
            (p.width, p.width / 2, p.width / 2),
        );
        if !self.initialized {
            // The wrapper initializes dimensions lazily, and forces a single
            // slice even with num_threads(4). Prime it privately, then enable
            // independent slices; the first published frame must be a new IDR.
            self.encoder.encode(&source)?;
            if p.width * p.height >= 65_536 {
                use openh264_sys2::{
                    ENCODER_OPTION_SVC_ENCODE_PARAM_EXT, SEncParamExt, SM_FIXEDSLCNUM_SLICE,
                };
                let mut params = std::mem::MaybeUninit::<SEncParamExt>::zeroed();
                // SAFETY: initialized encoder, correctly sized C parameter
                // storage, unchanged dimensions/color/layers and buffer API.
                // Fixed slice count is supported by this pinned upstream API.
                unsafe {
                    let api = self.encoder.raw_api();
                    if api.get_option(
                        ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        params.as_mut_ptr().cast(),
                    ) != 0
                    {
                        return Err("software encoder parameters unavailable".into());
                    }
                    let mut params = params.assume_init();
                    params.iMultipleThreadIdc = 4;
                    params.sSpatialLayers[0].sSliceArgument.uiSliceMode = SM_FIXEDSLCNUM_SLICE;
                    params.sSpatialLayers[0].sSliceArgument.uiSliceNum = 4;
                    if api.set_option(
                        ENCODER_OPTION_SVC_ENCODE_PARAM_EXT,
                        (&mut params as *mut SEncParamExt).cast(),
                    ) != 0
                    {
                        return Err("software encoder slice configuration failed".into());
                    }
                }
            }
            self.encoder.force_intra_frame();
            self.initialized = true;
        }
        self.packet.clear();
        self.encoder.encode(&source)?.write_vec(&mut self.packet);
        if self.packet.is_empty() || self.packet.len() > pab_protocol::DESKTOP_MAX_FRAME_BYTES {
            return Err("software encoder skipped a frame or exceeded the packet bound".into());
        }
        Ok(&self.packet)
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
        let pixels = dimensions(width, height)?;
        Ok(Self {
            decoder: Decoder::new()?,
            width,
            height,
            nv12: vec![0; pixels * 3 / 2],
        })
    }
    /// CPU NV12, tightly packed, limited BT.709 as required by the session.
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
        assert!(SoftwareEncoder::new(63, 48, 15).is_err());
        assert!(SoftwareDecoder::new(8192, 8192).is_err());
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
