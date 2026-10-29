//! Explicit remote-device native acceptance. Never run on the development PC.
use image::ImageEncoder;
use pab_desktop_media::clipboard::NativeClipboard;
use pab_protocol::*;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--worker") {
        std::process::exit(pab_desktop_media::worker_main(args.into_iter().skip(1)));
    }
    let machine = std::env::var("COMPUTERNAME").unwrap_or_default();
    if args.as_slice() != ["--run"]
        || (cfg!(windows) && !["WIN-RASS8RC6V3H", "WIN-RACIRV0PP7C"].contains(&machine.as_str()))
    {
        return Err("Run explicitly on the designated 90/Server03 test device only".into());
    }
    let board = NativeClipboard::new(std::env::current_exe()?, vec!["--worker".into()]);
    // Keep prior supported content only in memory, never in receipts or logs.
    let before = board
        .exchange(DesktopClipboardOperation::Read { after: None })
        .await?;
    let mut owned_revision = None;
    let result=async {
        let (width,height)=(1920u32,1080u32);
        let mut rgba=Vec::with_capacity((width*height*4) as usize);
        for y in 0..height {for x in 0..width {rgba.extend_from_slice(&[(x%256) as u8,(y%256) as u8,((x+y)%256) as u8,if x<width/2{255}else{128}]);}}
        let mut data=Vec::new();image::codecs::png::PngEncoder::new(&mut data).write_image(&rgba,width,height,image::ExtendedColorType::Rgba8)?;
        let image=DesktopClipboardImage{width,height,bytes:data.len() as u32,data};
        let started=std::time::Instant::now();
        let written=board.exchange(DesktopClipboardOperation::WriteImage{image:image.clone(),expected_revision:Some(before.revision)}).await?;
        owned_revision=Some(written.revision);
        // Independent owner forces a real GetClipboardData/decode, bypassing cache.
        let reader=NativeClipboard::new(std::env::current_exe()?,vec!["--worker".into()]);
        let read=reader.exchange(DesktopClipboardOperation::Read{after:None}).await?;
        let received=read.image.ok_or("no native image format")?;
        let decoded=image::load_from_memory_with_format(&received.data,image::ImageFormat::Png)?.to_rgba8();
        if decoded.as_raw()!=&rgba {return Err::<_,Box<dyn std::error::Error>>("pixel/alpha mismatch".into());}
        #[cfg(windows)] {
        let consumer=std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-STA","-Command",
            "Add-Type -AssemblyName System.Windows.Forms; $i=[System.Windows.Forms.Clipboard]::GetImage(); if($null -eq $i -or $i.Width -ne 1920 -or $i.Height -ne 1080){exit 2}; $p=$i.GetPixel(10,10); if($p.R -ne 10 -or $p.G -ne 10 -or $p.B -ne 20){exit 3}; $i.Dispose()"])
            .output()?;
        if !consumer.status.success(){return Err("Windows Forms image paste consumer failed".into());}
        }
        let unchanged=board.exchange(DesktopClipboardOperation::Read{after:Some(written.revision)}).await?;
        if unchanged.changed || unchanged.image.is_some(){return Err("echo suppression failed".into());}
        let bad=board.exchange(DesktopClipboardOperation::WriteImage{image:image.clone(),expected_revision:Some(written.revision.wrapping_sub(1))}).await;
        if bad.is_ok(){return Err("stale revision write was accepted".into());}
        let text=board.exchange(DesktopClipboardOperation::Write{text:"Pixels image clipboard test: 中文 🖥️\nsecond line".into()}).await?;
        owned_revision=Some(text.revision);
        let read=reader.exchange(DesktopClipboardOperation::Read{after:None}).await?;
        if read.text.as_deref()!=Some("Pixels image clipboard test: 中文 🖥️\nsecond line") || read.image.is_some(){return Err("image to text transition failed".into());}
        // Writing from a second process also verifies the first owner's message
        // loop responds while its command pipe is idle.
        reader.exchange(DesktopClipboardOperation::WriteImage{image,expected_revision:Some(read.revision)}).await?;
        let final_read=board.exchange(DesktopClipboardOperation::Read{after:None}).await?;
        owned_revision=Some(final_read.revision);
        if final_read.image.is_none(){return Err("text to image transition failed".into());}
        #[cfg(windows)] {
        let screenshot=std::process::Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-STA","-Command",
            "Add-Type -AssemblyName System.Windows.Forms; $b=New-Object System.Drawing.Bitmap 64,32; $g=[System.Drawing.Graphics]::FromImage($b); $g.Clear([System.Drawing.Color]::Red); [System.Windows.Forms.Clipboard]::SetImage($b); $g.Dispose(); $b.Dispose()"])
            .output()?;
        if !screenshot.status.success(){return Err("Windows bitmap copy fixture failed".into());}
        let bitmap=board.exchange(DesktopClipboardOperation::Read{after:None}).await?;
        owned_revision=Some(bitmap.revision);
        let bitmap=bitmap.image.ok_or("Windows bitmap copy was not readable")?;
        let pixels=image::load_from_memory_with_format(&bitmap.data,image::ImageFormat::Png)?.to_rgba8();
        if pixels.dimensions()!=(64,32) || pixels.get_pixel(10,10).0!=[255,0,0,255]{return Err("Windows bitmap copy color mismatch".into());}
        }
        Ok(serde_json::json!({"pass":true,"machine":machine,"width":width,"height":height,"png_bytes":received.bytes,
            "elapsed_ms":started.elapsed().as_millis(),"native_read":true,"alpha_pixels":true,"no_echo":true,"stale_write_rejected":true,"text_regression":true,"owner_message_loop":true,"windows_forms_paste":cfg!(windows),"windows_bitmap_copy":cfg!(windows)}))
    }.await;
    // Restore only if the test still owns the clipboard; preserve concurrent user copies.
    let current = board
        .exchange(DesktopClipboardOperation::Read {
            after: owned_revision,
        })
        .await?;
    let restored = if Some(current.revision) == owned_revision {
        let operation = if let Some(image) = before.image {
            Some(DesktopClipboardOperation::WriteImage {
                image,
                expected_revision: owned_revision,
            })
        } else {
            before
                .text
                .map(|text| DesktopClipboardOperation::Write { text })
        };
        if let Some(operation) = operation {
            board.exchange(operation).await?;
            true
        } else {
            false
        }
    } else {
        false
    };
    let mut report = result?;
    report["previous_supported_content_restored"] = restored.into();
    println!("{}", report);
    Ok(())
}
