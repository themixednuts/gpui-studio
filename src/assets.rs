//! Image assets copied into a project.
//!
//! Imported images live in `artboards/assets/`, so `<img src="assets/…">`
//! resolves the same way on the canvas and in a browser opening the artboard
//! file.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, bail};

/// Largest image Studio imports.
pub const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// File extensions Studio imports as images.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp"];

/// Whether a path looks like an importable image.
#[must_use]
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Lowercase extension of a file name (`"photo.JPG"` → `"jpg"`).
#[must_use]
pub fn extension(file_name: &str) -> Option<String> {
    Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// Intrinsic size in CSS pixels.
#[must_use]
pub fn image_size(bytes: &[u8], extension: &str) -> Option<(f32, f32)> {
    if extension == "svg" {
        return svg_size(std::str::from_utf8(bytes).ok()?);
    }
    let size = imagesize::blob_size(bytes).ok()?;
    Some((size.width as f32, size.height as f32))
}

fn svg_attr(tag: &str, name: &str) -> Option<String> {
    let key = format!(" {name}=");
    let start = tag.find(&key)? + key.len();
    let quote = tag[start..].chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &tag[start + 1..];
    let end = rest.find(quote)?;
    Some(rest[..end].to_owned())
}

fn svg_size(source: &str) -> Option<(f32, f32)> {
    let start = source.find("<svg")?;
    let tag = &source[start..start + source[start..].find('>')?];
    let number = |v: String| v.trim().trim_end_matches("px").parse::<f32>().ok();
    let width = svg_attr(tag, "width").and_then(number);
    let height = svg_attr(tag, "height").and_then(number);
    if let (Some(w), Some(h)) = (width, height) {
        return Some((w, h));
    }
    let view_box: Vec<f32> = svg_attr(tag, "viewBox")?
        .split([' ', ','])
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect();
    match view_box.as_slice() {
        [_, _, w, h] if *w > 0.0 && *h > 0.0 => Some((*w, *h)),
        _ => None,
    }
}

/// Scale a size down (never up) to fit within `max` on both axes.
#[must_use]
pub fn fit_within((w, h): (f32, f32), max: f32) -> (f32, f32) {
    let scale = (max / w.max(1.0)).min(max / h.max(1.0)).min(1.0);
    ((w * scale).round().max(1.0), (h * scale).round().max(1.0))
}

fn slug(stem: &str) -> String {
    let mut out = String::new();
    for ch in stem.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_owned();
    if out.is_empty() {
        "image".to_owned()
    } else {
        out
    }
}

/// Copy image bytes into `artboards/assets/`, returning the `src` to use from
/// an artboard file. Identical content under the same name is reused.
pub fn store_image(artboards_dir: &Path, file_name: &str, bytes: &[u8]) -> Result<String> {
    if bytes.is_empty() {
        bail!("the image is empty");
    }
    if bytes.len() > MAX_IMAGE_BYTES {
        bail!("images larger than 32 MB are not imported");
    }
    let ext = extension(file_name).context("the image has no file extension")?;
    let ext = if ext == "jpeg" { "jpg".to_owned() } else { ext };
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        bail!("unsupported image type .{ext}");
    }
    let stem = Path::new(file_name)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image");
    let dir = artboards_dir.join("assets");
    fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let base = slug(stem);
    for n in 0..10_000 {
        let name = if n == 0 {
            format!("{base}.{ext}")
        } else {
            format!("{base}-{n}.{ext}")
        };
        let path = dir.join(&name);
        match fs::read(&path) {
            Ok(existing) if existing == bytes => return Ok(format!("assets/{name}")),
            Ok(_) => continue,
            Err(_) => {
                fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
                return Ok(format!("assets/{name}"));
            }
        }
    }
    bail!("too many images named {base}")
}

/// A 1×1 PNG for tests.
#[cfg(test)]
#[must_use]
pub(crate) fn tests_png() -> Vec<u8> {
    tests::PNG.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1×1 transparent PNG.
    pub(crate) const PNG: &[u8] = &[
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f,
        0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0d, 0x0a, 0x2d, 0xb4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn stores_dedupes_and_measures() {
        let dir = tempfile::tempdir().unwrap();
        let a = store_image(dir.path(), "My Photo.PNG", PNG).unwrap();
        assert_eq!(a, "assets/my-photo.png");
        assert_eq!(store_image(dir.path(), "My Photo.PNG", PNG).unwrap(), a);
        let mut other = PNG.to_vec();
        other.push(0);
        assert_eq!(
            store_image(dir.path(), "my photo.png", &other).unwrap(),
            "assets/my-photo-1.png"
        );
        assert!(store_image(dir.path(), "notes.txt", b"hi").is_err());
        assert_eq!(image_size(PNG, "png"), Some((1.0, 1.0)));
        assert_eq!(
            image_size(
                br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 12">"#,
                "svg"
            ),
            Some((24.0, 12.0))
        );
        assert_eq!(
            image_size(br#"<svg width="40px" height='20'>"#, "svg"),
            Some((40.0, 20.0))
        );
        assert_eq!(fit_within((4000.0, 2000.0), 1000.0), (1000.0, 500.0));
        assert_eq!(fit_within((10.0, 20.0), 1000.0), (10.0, 20.0));
        assert!(is_image_path(Path::new("/x/a.JPeG")));
    }
}
