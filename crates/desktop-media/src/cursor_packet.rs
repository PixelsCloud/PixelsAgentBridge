//! Private native cursor command shared by platform renderers.
use pab_protocol::*;
use pab_remote_desktop::CursorUpdate;

pub(crate) fn encode(config: &DesktopVideoConfig, update: &CursorUpdate) -> Vec<u8> {
    let mut packet = Vec::with_capacity(44 + update.bytes.len());
    packet.push(1);
    packet.push(u8::from(update.header.position.is_some()));
    let (x, y) = update
        .header
        .position
        .map(|point| {
            (
                i64::from(point.x) - i64::from(config.screen.x),
                i64::from(point.y) - i64::from(config.screen.y),
            )
        })
        .unwrap_or((0, 0));
    packet.extend_from_slice(&x.to_le_bytes());
    packet.extend_from_slice(&y.to_le_bytes());
    packet.extend_from_slice(&config.screen.desktop_width.to_le_bytes());
    packet.extend_from_slice(&config.screen.desktop_height.to_le_bytes());
    match &update.header.image {
        None => packet.push(0),
        Some(image) => {
            packet.push(match image.format {
                DesktopCursorFormat::PremultipliedBgra => 1,
                DesktopCursorFormat::AndXorBgra => 2,
            });
            for value in [image.width, image.height, image.hotspot_x, image.hotspot_y] {
                packet.extend_from_slice(&value.to_le_bytes());
            }
            packet.extend_from_slice(&update.bytes);
        }
    }
    packet
}
