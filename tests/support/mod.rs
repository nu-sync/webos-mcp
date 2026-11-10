//! Shared helpers for integration tests. Not every test uses every helper.
#![allow(dead_code)]

pub mod simulated_tv;

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

/// A tool result reduced to what must match between front ends. Durations and
/// uptime differ between any two processes, and the metrics text block repeats
/// them, so those are removed; image payloads are compared by type only.
pub fn comparable(tool: &str, mut result: serde_json::Value) -> serde_json::Value {
    fn scrub(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(map) => {
                for (key, entry) in map.iter_mut() {
                    if key.ends_with("_ms") || key == "since_start_seconds" {
                        *entry = serde_json::Value::Null;
                    } else {
                        scrub(entry);
                    }
                }
            }
            serde_json::Value::Array(list) => list.iter_mut().for_each(scrub),
            _ => {}
        }
    }
    // The server writes `resultType` on the wire; the client library parses it
    // and does not write it back when a parsed result is serialized again.
    if let Some(map) = result.as_object_mut() {
        map.remove("resultType");
    }
    if tool == "tv_metrics" {
        scrub(&mut result["structuredContent"]);
        result["content"] = serde_json::Value::Null;
    }
    if let Some(blocks) = result["content"].as_array_mut() {
        for block in blocks.iter_mut().filter(|block| block["type"] == "image") {
            *block = serde_json::json!({"type": "image", "mimeType": block["mimeType"]});
        }
    }
    result
}

/// `comparable`, plus what differs between any two connected sessions:
/// observation IDs are random and capture timings vary. Results that carry
/// metadata repeat it in a text block, which is dropped for the same reason.
pub fn comparable_session(tool: &str, result: serde_json::Value) -> serde_json::Value {
    let mut result = comparable(tool, result);
    if let Some(id) = result.pointer_mut("/structuredContent/observation_id") {
        assert_eq!(id.as_str().map(str::len), Some(32), "observation ID shape");
        *id = serde_json::Value::Null;
    }
    for field in ["elapsed_ms", "returned_at_unix_ms"] {
        if let Some(value) = result.pointer_mut(&format!("/structuredContent/{field}")) {
            assert!(value.is_u64(), "{field} must be a number");
            *value = serde_json::Value::Null;
        }
    }
    if result
        .get("structuredContent")
        .is_some_and(|content| !content.is_null())
        && let Some(blocks) = result["content"].as_array_mut()
    {
        blocks.retain(|block| block["type"] == "image");
    }
    result
}
