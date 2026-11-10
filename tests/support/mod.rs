//! Shared helpers for opt-in live tests.

/// Fully decode a returned screenshot and report its pixel dimensions.
pub fn decoded_dimensions(
    mime_type: &str,
    bytes: &[u8],
) -> Result<(u32, u32), Box<dyn std::error::Error>> {
    const LIMIT: usize = 64 * 1024 * 1024;
    match mime_type {
        "image/png" => {
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info()?;
            let size = reader.output_buffer_size().ok_or("PNG size unavailable")?;
            if size > LIMIT {
                return Err("screenshot exceeds the test decode limit".into());
            }
            let mut pixels = vec![0; size];
            let frame = reader.next_frame(&mut pixels)?;
            reader.finish()?;
            Ok((frame.width, frame.height))
        }
        "image/jpeg" => {
            use image::ImageDecoder;
            let decoder = image::codecs::jpeg::JpegDecoder::new(std::io::Cursor::new(bytes))?;
            let dimensions = decoder.dimensions();
            let size = usize::try_from(decoder.total_bytes())?;
            if size > LIMIT {
                return Err("screenshot exceeds the test decode limit".into());
            }
            let mut pixels = vec![0; size];
            decoder.read_image(&mut pixels)?;
            Ok(dimensions)
        }
        _ => Err("unexpected screenshot MIME type".into()),
    }
}

/// The evidence path with the extension matching the returned encoding.
pub fn evidence_path(path: &std::path::Path, mime_type: &str) -> std::path::PathBuf {
    path.with_extension(if mime_type == "image/jpeg" {
        "jpg"
    } else {
        "png"
    })
}
