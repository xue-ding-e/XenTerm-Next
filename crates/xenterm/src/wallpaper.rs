//! Bounded, GUI-independent static wallpaper assets and transforms.
//!
//! Decode/process on a worker, cache the result in the UI, and composite opacity
//! and the dark overlay at paint time. No timer or frame-driven decoding is needed.

use anyhow::{bail, Context, Result};
use image::{imageops, DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Cursor, Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
pub const MAX_DECODE_PIXELS: u64 = 32 * 1024 * 1024;
/// Limit CPU transforms/GPU uploads independently of compressed input size.
pub const MAX_PROCESSED_PIXELS: u64 = 4 * 1024 * 1024;
const MAX_EDGE: u32 = 16_384;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fit {
    #[default]
    Cover,
    Contain,
    Stretch,
    Original,
    Tile,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    TerminalPane,
    ContinuousTerminalCanvas,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WallpaperSettings {
    pub enabled: bool,
    pub source: String,
    pub fit: Fit,
    pub scope: Scope,
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub offset_x: f32,
    pub offset_y: f32,
    pub zoom: f32,
    /// Clockwise degrees, applied after crop and flips.
    pub rotation: f32,
    pub flip_x: bool,
    pub flip_y: bool,
    /// Normalized x, y, width, height in the EXIF-oriented source.
    pub crop: [f32; 4],
    pub opacity: f32,
    pub overlay: f32,
    /// Multipliers; 1 is unchanged, 0 is black/grayscale respectively.
    pub brightness: f32,
    pub saturation: f32,
    pub blur: f32,
}
impl Default for WallpaperSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            source: String::new(),
            fit: Fit::Cover,
            scope: Scope::TerminalPane,
            anchor_x: 0.5,
            anchor_y: 0.5,
            offset_x: 0.,
            offset_y: 0.,
            zoom: 1.,
            rotation: 0.,
            flip_x: false,
            flip_y: false,
            crop: [0., 0., 1., 1.],
            opacity: 0.25,
            overlay: 0.35,
            brightness: 1.,
            saturation: 1.,
            blur: 0.,
        }
    }
}
fn finite(value: f32, fallback: f32, min: f32, max: f32) -> f32 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}
impl WallpaperSettings {
    /// Normalize untrusted/old configuration before arithmetic or allocating.
    pub fn normalized(&self) -> Self {
        let mut s = self.clone();
        s.anchor_x = finite(s.anchor_x, 0.5, 0., 1.);
        s.anchor_y = finite(s.anchor_y, 0.5, 0., 1.);
        s.offset_x = finite(s.offset_x, 0., -100_000., 100_000.);
        s.offset_y = finite(s.offset_y, 0., -100_000., 100_000.);
        s.zoom = finite(s.zoom, 1., 0.1, 10.);
        s.rotation = if s.rotation.is_finite() {
            s.rotation.rem_euclid(360.)
        } else {
            0.
        };
        s.crop[0] = finite(s.crop[0], 0., 0., 1.);
        s.crop[1] = finite(s.crop[1], 0., 0., 1.);
        s.crop[2] = finite(s.crop[2], 1., 0., 1.).min(1. - s.crop[0]);
        s.crop[3] = finite(s.crop[3], 1., 0., 1.).min(1. - s.crop[1]);
        // An empty/invalid crop resets safely rather than allocating a zero image.
        if s.crop[2] <= 0. || s.crop[3] <= 0. {
            s.crop = [0., 0., 1., 1.];
        }
        s.opacity = finite(s.opacity, 0.25, 0., 1.);
        s.overlay = finite(s.overlay, 0.35, 0., 1.);
        s.brightness = finite(s.brightness, 1., 0., 2.);
        s.saturation = finite(s.saturation, 1., 0., 2.);
        s.blur = finite(s.blur, 0., 0., 30.);
        s
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Layout {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub tile: bool,
}
/// Layout the already transformed source. Clip to viewport in the render adapter.
/// For tiles, x/y are the phase of a repeating grid, not just one visible tile.
pub fn geometry(source: (u32, u32), bounds: (f32, f32), settings: &WallpaperSettings) -> Layout {
    let s = settings.normalized();
    let (sw, sh) = (source.0 as f32, source.1 as f32);
    let (bw, bh) = bounds;
    if sw <= 0.
        || sh <= 0.
        || !bw.is_finite()
        || !bh.is_finite()
        || bw <= 0.
        || bh <= 0.
        || bw > 1_000_000.
        || bh > 1_000_000.
    {
        return Layout::default();
    }
    let (w, h) = match s.fit {
        Fit::Stretch => (bw, bh),
        Fit::Original | Fit::Tile => (sw, sh),
        Fit::Cover | Fit::Contain => {
            let ratios = (bw / sw, bh / sh);
            let scale = if s.fit == Fit::Cover {
                ratios.0.max(ratios.1)
            } else {
                ratios.0.min(ratios.1)
            };
            (sw * scale, sh * scale)
        }
    };
    let (w, h) = (w * s.zoom, h * s.zoom);
    Layout {
        x: (bw - w) * s.anchor_x + s.offset_x,
        y: (bh - h) * s.anchor_y + s.offset_y,
        width: w,
        height: h,
        tile: s.fit == Fit::Tile,
    }
}

fn read_bounded(path: &Path) -> Result<Vec<u8>> {
    // Reject pipes/devices before opening: opening a FIFO may otherwise block a
    // worker indefinitely. Recheck the opened handle below for replacement races.
    let before = fs::metadata(path).context("Cannot inspect wallpaper image")?;
    if !before.is_file() || before.len() > MAX_FILE_BYTES {
        bail!("Wallpaper must be a regular file of at most 32 MiB");
    }
    let mut file = File::open(path).context("Cannot open wallpaper image")?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() > MAX_FILE_BYTES {
        bail!("Wallpaper must be a regular file of at most 32 MiB");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        bail!("Wallpaper exceeds 32 MiB");
    }
    Ok(bytes)
}
fn format(bytes: &[u8]) -> Result<ImageFormat> {
    let format = image::guess_format(bytes).context("Unrecognized wallpaper format")?;
    match format {
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Bmp => Ok(format),
        _ => bail!("Wallpaper supports only PNG, JPEG, WebP and BMP"),
    }
}
/// The upstream WebP metadata reader allocates from declared chunk lengths, so
/// validate the entire RIFF envelope before it is allowed to inspect EXIF.
fn validate_webp(bytes: &[u8]) -> Result<()> {
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WEBP" {
        bail!("Invalid WebP container");
    }
    let declared = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
    if declared.checked_add(8) != Some(bytes.len()) {
        bail!("Invalid WebP container size");
    }
    let mut offset = 12usize;
    let mut canvas = None;
    let mut frame = None;
    while offset < bytes.len() {
        let header_end = offset.checked_add(8).context("WebP chunk overflow")?;
        if header_end > bytes.len() {
            bail!("Truncated WebP chunk");
        }
        let tag = &bytes[offset..offset + 4];
        let length = u32::from_le_bytes(bytes[offset + 4..header_end].try_into().unwrap()) as usize;
        let end = header_end
            .checked_add(length)
            .context("WebP chunk overflow")?;
        let padded_end = end
            .checked_add(length & 1)
            .context("WebP padding overflow")?;
        if padded_end > bytes.len() {
            bail!("WebP chunk exceeds file bounds");
        }
        // Animated images require nested frame parsing and are outside this
        // static-wallpaper feature; reject rather than allocating frame data.
        if tag == b"ANIM"
            || tag == b"ANMF"
            || (tag == b"VP8X" && length > 0 && bytes[header_end] & 2 != 0)
        {
            bail!("Animated WebP wallpapers are not supported");
        }
        let payload = &bytes[header_end..end];
        let dimensions = match tag {
            b"VP8X" => {
                if length != 10 || canvas.is_some() {
                    bail!("Invalid WebP extended header");
                }
                let w = 1 + u32::from_le_bytes([payload[4], payload[5], payload[6], 0]);
                let h = 1 + u32::from_le_bytes([payload[7], payload[8], payload[9], 0]);
                canvas = Some((w, h));
                Some((w, h))
            }
            b"VP8 " => {
                if length < 10
                    || payload[0] & 1 != 0
                    || &payload[3..6] != b"\x9d\x01\x2a"
                    || frame.is_some()
                {
                    bail!("Invalid WebP VP8 frame header");
                }
                let w = u32::from(u16::from_le_bytes([payload[6], payload[7]]) & 0x3fff);
                let h = u32::from(u16::from_le_bytes([payload[8], payload[9]]) & 0x3fff);
                frame = Some((w, h));
                Some((w, h))
            }
            b"VP8L" => {
                if length < 5 || payload[0] != 0x2f || frame.is_some() {
                    bail!("Invalid WebP VP8L frame header");
                }
                let bits = u32::from_le_bytes(payload[1..5].try_into().unwrap());
                let w = (bits & 0x3fff) + 1;
                let h = ((bits >> 14) & 0x3fff) + 1;
                frame = Some((w, h));
                Some((w, h))
            }
            _ => None,
        };
        if let Some((w, h)) = dimensions {
            if w == 0
                || h == 0
                || w > MAX_EDGE
                || h > MAX_EDGE
                || u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS
            {
                bail!("WebP frame exceeds wallpaper pixel limits");
            }
        }
        offset = padded_end;
    }
    let frame = frame.context("WebP contains no static image frame")?;
    if canvas.is_some_and(|canvas| canvas != frame) {
        bail!("WebP frame dimensions differ from its canvas");
    }
    Ok(())
}
fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let format = format(bytes)?;
    if format == ImageFormat::WebP {
        validate_webp(bytes)?;
    }
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder().context("Cannot decode wallpaper")?;
    let (w, h) = decoder.dimensions();
    if w == 0
        || h == 0
        || u64::from(w) * u64::from(h) > MAX_DECODE_PIXELS
        || decoder.total_bytes() > 256 * 1024 * 1024
    {
        bail!("Wallpaper exceeds the 32 megapixel decode limit");
    }
    let orientation = decoder.orientation().context("Invalid image orientation")?;
    let mut image = DynamicImage::from_decoder(decoder).context("Corrupt wallpaper image")?;
    image.apply_orientation(orientation);
    Ok(image)
}

/// Import a validated original without re-encoding; its name is a content hash.
/// No-clobber atomic publication allows concurrent imports without partial assets.
pub fn import_asset(path: &Path, managed_dir: &Path) -> Result<PathBuf> {
    let bytes = read_bounded(path)?;
    let fmt = format(&bytes)?;
    drop(decode(&bytes)?);
    fs::create_dir_all(managed_dir).context("Cannot create wallpaper asset directory")?;
    let directory = managed_dir.canonicalize()?;
    let ext = match fmt {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::WebP => "webp",
        ImageFormat::Bmp => "bmp",
        _ => unreachable!(),
    };
    let destination = directory.join(format!("{:x}.{ext}", Sha256::digest(&bytes)));
    let mut temp = tempfile::NamedTempFile::new_in(&directory)?;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    match temp.persist_noclobber(&destination) {
        Ok(_) => (),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Refuse symlinks and modified assets rather than silently trusting names.
            if fs::symlink_metadata(&destination)?.file_type().is_symlink()
                || read_bounded(&destination)? != bytes
            {
                bail!("Existing wallpaper asset does not match its content hash");
            }
        }
        Err(error) => return Err(error.error).context("Cannot publish wallpaper asset"),
    }
    Ok(destination)
}

/// Resolve only a managed content-hash basename, never a path from imported config.
pub fn resolve_asset(managed_dir: &Path, name: &str) -> Result<PathBuf> {
    let (hash, extension) = name
        .rsplit_once('.')
        .context("Invalid wallpaper asset name")?;
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        || !["png", "jpg", "webp", "bmp"].contains(&extension)
    {
        bail!("Invalid wallpaper asset name");
    }
    let root = managed_dir.canonicalize()?;
    let path = root.join(name);
    if fs::symlink_metadata(&path)?.file_type().is_symlink()
        || path.canonicalize()?.parent() != Some(root.as_path())
    {
        bail!("Wallpaper asset must stay inside the managed directory");
    }
    let digest = format!("{:x}", Sha256::digest(read_bounded(&path)?));
    if digest != hash {
        bail!("Wallpaper asset does not match its content hash");
    }
    Ok(path)
}

/// Process only on import/settings change, never inside a render callback.
/// Returned pixels are straight-alpha RGBA. Opacity/overlay are composited by UI.
pub struct ProcessedImage {
    pub pixels: RgbaImage,
    /// Original transformed pixel dimensions, independent of upload resolution.
    pub logical_size: (u32, u32),
}

pub fn load_prepared(path: &Path, settings: &WallpaperSettings) -> Result<ProcessedImage> {
    let source = decode(&read_bounded(path)?)?;
    let logical_size = transformed_size((source.width(), source.height()), settings);
    Ok(ProcessedImage {
        pixels: process(source, settings),
        logical_size,
    })
}

pub fn load_processed(path: &Path, settings: &WallpaperSettings) -> Result<RgbaImage> {
    Ok(load_prepared(path, settings)?.pixels)
}

fn crop_rect(source: (u32, u32), s: &WallpaperSettings) -> (u32, u32, u32, u32) {
    let (w, h) = source;
    let x = ((w as f32 * s.crop[0]).floor() as u32).min(w - 1);
    let y = ((h as f32 * s.crop[1]).floor() as u32).min(h - 1);
    let cw = ((w as f32 * s.crop[2]).ceil() as u32).max(1).min(w - x);
    let ch = ((h as f32 * s.crop[3]).ceil() as u32).max(1).min(h - y);
    (x, y, cw, ch)
}

/// Logical dimensions after crop and rotation, before bounded GPU resizing.
pub fn transformed_size(source: (u32, u32), settings: &WallpaperSettings) -> (u32, u32) {
    if source.0 == 0 || source.1 == 0 {
        return (0, 0);
    }
    let s = settings.normalized();
    let (_, _, w, h) = crop_rect(source, &s);
    if s.rotation == 0. || s.rotation == 180. {
        return (w, h);
    }
    if s.rotation == 90. || s.rotation == 270. {
        return (h, w);
    }
    let (sin, cos) = f64::from(s.rotation).to_radians().sin_cos();
    (
        (f64::from(w) * cos.abs() + f64::from(h) * sin.abs()).ceil() as u32,
        (f64::from(w) * sin.abs() + f64::from(h) * cos.abs()).ceil() as u32,
    )
}

fn process(source: DynamicImage, settings: &WallpaperSettings) -> RgbaImage {
    let s = settings.normalized();
    let (x, y, cw, ch) = crop_rect((source.width(), source.height()), &s);
    let mut pixels = source.crop_imm(x, y, cw, ch).into_rgba8();
    drop(source);
    // Premultiplied filtering avoids colored halos around transparent PNG edges.
    let filtered = u64::from(cw) * u64::from(ch) > MAX_PROCESSED_PIXELS
        || cw.max(ch) > 4096
        || s.rotation % 90. != 0.
        || s.blur > 0.;
    if filtered {
        premultiply(&mut pixels);
    }
    pixels = bounded_resize(pixels, MAX_PROCESSED_PIXELS);
    if s.flip_x {
        imageops::flip_horizontal_in_place(&mut pixels);
    }
    if s.flip_y {
        imageops::flip_vertical_in_place(&mut pixels);
    }
    pixels = rotate(&pixels, s.rotation);
    if s.blur > 0. {
        pixels = imageops::blur(&pixels, s.blur);
    }
    if filtered {
        unpremultiply(&mut pixels);
    }
    if s.brightness != 1. || s.saturation != 1. {
        for p in pixels.pixels_mut() {
            let gray = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
            for c in &mut p.0[..3] {
                *c = ((gray + (*c as f32 - gray) * s.saturation) * s.brightness)
                    .clamp(0., 255.)
                    .round() as u8;
            }
        }
    }
    pixels
}
fn premultiply(image: &mut RgbaImage) {
    for p in image.pixels_mut() {
        let alpha = u32::from(p[3]);
        for c in &mut p.0[..3] {
            *c = ((u32::from(*c) * alpha + 127) / 255) as u8;
        }
    }
}
fn unpremultiply(image: &mut RgbaImage) {
    for p in image.pixels_mut() {
        let alpha = u32::from(p[3]);
        for c in &mut p.0[..3] {
            *c = if alpha == 0 {
                0
            } else {
                ((u32::from(*c) * 255 + alpha / 2) / alpha).min(255) as u8
            };
        }
    }
}
fn bounded_resize(image: RgbaImage, budget: u64) -> RgbaImage {
    let (w, h) = image.dimensions();
    let scale = ((budget as f64 / (u64::from(w) * u64::from(h)) as f64).sqrt())
        .min(4096. / f64::from(w.max(h)))
        .min(1.);
    if scale == 1. {
        image
    } else {
        imageops::resize(
            &image,
            (f64::from(w) * scale).floor().max(1.) as u32,
            (f64::from(h) * scale).floor().max(1.) as u32,
            imageops::FilterType::Triangle,
        )
    }
}
fn rotate(image: &RgbaImage, degrees: f32) -> RgbaImage {
    if degrees == 0. {
        return image.clone();
    }
    if degrees == 90. {
        return imageops::rotate90(image);
    }
    if degrees == 180. {
        return imageops::rotate180(image);
    }
    if degrees == 270. {
        return imageops::rotate270(image);
    }
    let radians = f64::from(degrees).to_radians();
    let (sin, cos) = radians.sin_cos();
    let (w, h) = (f64::from(image.width()), f64::from(image.height()));
    let natural_w = (w * cos.abs() + h * sin.abs()).ceil();
    let natural_h = (w * sin.abs() + h * cos.abs()).ceil();
    // Long, thin sources can have enormous rotated bounding boxes. Sample the
    // bounded destination directly instead of allocating the natural rectangle.
    let scale = (MAX_PROCESSED_PIXELS as f64 / (natural_w * natural_h))
        .sqrt()
        .min(4096. / natural_w.max(natural_h))
        .min(1.);
    let ow = (natural_w * scale).floor().max(1.) as u32;
    let oh = (natural_h * scale).floor().max(1.) as u32;
    RgbaImage::from_fn(ow, oh, |x, y| {
        let dx = (f64::from(x) + 0.5 - f64::from(ow) / 2.) / scale;
        let dy = (f64::from(y) + 0.5 - f64::from(oh) / 2.) / scale;
        sample(
            image,
            cos * dx + sin * dy + w / 2. - 0.5,
            -sin * dx + cos * dy + h / 2. - 0.5,
        )
    })
}
fn sample(image: &RgbaImage, x: f64, y: f64) -> Rgba<u8> {
    let (ix, iy) = (x.floor() as i64, y.floor() as i64);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let mut channels = [0.; 4];
    for (dx, wx) in [(0, 1. - fx), (1, fx)] {
        for (dy, wy) in [(0, 1. - fy), (1, fy)] {
            let (sx, sy) = (ix + dx, iy + dy);
            if sx >= 0 && sy >= 0 && sx < i64::from(image.width()) && sy < i64::from(image.height())
            {
                for (out, value) in channels
                    .iter_mut()
                    .zip(image.get_pixel(sx as u32, sy as u32).0)
                {
                    *out += f64::from(value) * wx * wy;
                }
            }
        }
    }
    Rgba(channels.map(|v| v.round().clamp(0., 255.) as u8))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn colored() -> RgbaImage {
        RgbaImage::from_fn(3, 2, |x, y| Rgba([(x + y * 3) as u8 * 30, 0, 0, 255]))
    }
    #[test]
    fn defaults_are_opt_in_and_old_configs_work() {
        let s: WallpaperSettings = serde_json::from_str("{}").unwrap();
        assert!(!s.enabled);
        assert_eq!(s, WallpaperSettings::default());
        assert_eq!(
            serde_json::to_string(&Scope::ContinuousTerminalCanvas).unwrap(),
            "\"continuous_terminal_canvas\""
        );
    }
    #[test]
    fn normalization_handles_nan_infinity_and_empty_crop() {
        let s = WallpaperSettings {
            zoom: f32::NAN,
            opacity: f32::INFINITY,
            rotation: -90.,
            crop: [1., 1., -1., 0.],
            anchor_x: -2.,
            blur: 500.,
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.zoom, 1.);
        assert_eq!(s.opacity, 0.25);
        assert_eq!(s.rotation, 270.);
        assert_eq!(s.crop, [0., 0., 1., 1.]);
        assert_eq!(s.anchor_x, 0.);
        assert_eq!(s.blur, 30.);
    }
    #[test]
    fn crop_clamps_to_remaining_area() {
        let s = WallpaperSettings {
            crop: [0.5, 0.25, 1., 1.],
            ..Default::default()
        }
        .normalized();
        assert_eq!(s.crop, [0.5, 0.25, 0.5, 0.75]);
        let tiny = WallpaperSettings {
            crop: [0.999, 0.999, 0.001, 0.001],
            ..Default::default()
        };
        assert_eq!(
            process(DynamicImage::ImageRgba8(colored()), &tiny).dimensions(),
            (1, 1)
        );
    }
    #[test]
    fn fit_modes_anchor_offset_and_zoom() {
        let mut s = WallpaperSettings::default();
        assert_eq!(
            geometry((200, 100), (100., 100.), &s),
            Layout {
                x: -50.,
                y: 0.,
                width: 200.,
                height: 100.,
                tile: false
            }
        );
        s.fit = Fit::Contain;
        assert_eq!(
            geometry((200, 100), (100., 100.), &s),
            Layout {
                x: 0.,
                y: 25.,
                width: 100.,
                height: 50.,
                tile: false
            }
        );
        s.fit = Fit::Stretch;
        assert_eq!(geometry((200, 100), (100., 100.), &s).height, 100.);
        s.fit = Fit::Original;
        s.anchor_x = 1.;
        s.anchor_y = 0.;
        s.offset_x = 4.;
        s.offset_y = 7.;
        s.zoom = 2.;
        assert_eq!(
            geometry((200, 100), (100., 100.), &s),
            Layout {
                x: -296.,
                y: 7.,
                width: 400.,
                height: 200.,
                tile: false
            }
        );
        s.fit = Fit::Tile;
        assert!(geometry((200, 100), (100., 100.), &s).tile);
    }
    #[test]
    fn geometry_rejects_empty_or_nonfinite_bounds() {
        for dims in [
            (0., 100.),
            (f32::NAN, 100.),
            (100., f32::INFINITY),
            (f32::MAX, 1.),
        ] {
            assert_eq!(
                geometry((200, 100), dims, &WallpaperSettings::default()),
                Layout::default()
            );
        }
        assert_eq!(
            geometry((0, 10), (100., 100.), &WallpaperSettings::default()),
            Layout::default()
        );
    }
    #[test]
    fn all_nine_anchors_match_endpoints() {
        for x in [0., 0.5, 1.] {
            for y in [0., 0.5, 1.] {
                let s = WallpaperSettings {
                    fit: Fit::Original,
                    anchor_x: x,
                    anchor_y: y,
                    ..Default::default()
                };
                let l = geometry((20, 10), (100., 100.), &s);
                assert_eq!((l.x, l.y), (80. * x, 90. * y));
            }
        }
    }
    #[test]
    fn quarter_turn_and_flip_are_pixel_exact() {
        let original = colored();
        let s = WallpaperSettings {
            rotation: 90.,
            flip_x: true,
            ..Default::default()
        };
        assert_eq!(
            process(DynamicImage::ImageRgba8(original.clone()), &s),
            imageops::rotate90(&imageops::flip_horizontal(&original))
        );
    }
    #[test]
    fn crop_occurs_before_rotation() {
        let original = colored();
        let s = WallpaperSettings {
            crop: [0., 0., 1. / 3., 1.],
            rotation: 90.,
            ..Default::default()
        };
        let result = process(DynamicImage::ImageRgba8(original), &s);
        assert_eq!(result.dimensions(), (2, 1));
        assert_eq!(result.get_pixel(0, 0)[0], 90);
    }
    #[test]
    fn arbitrary_rotation_preserves_transparency_and_budget() {
        let image = RgbaImage::from_pixel(2, 2, Rgba([255, 0, 0, 128]));
        let s = WallpaperSettings {
            rotation: 45.,
            ..Default::default()
        };
        let output = process(DynamicImage::ImageRgba8(image), &s);
        assert!(output.pixels().any(|p| p[3] < 128));
        assert!(output
            .pixels()
            .filter(|p| p[3] > 0)
            .all(|p| p[0] >= 250 && p[1] == 0));
        let thin = RgbaImage::from_pixel(4096, 1, Rgba([0, 0, 0, 255]));
        let output = rotate(&thin, 45.);
        assert!(u64::from(output.width()) * u64::from(output.height()) <= MAX_PROCESSED_PIXELS);
    }
    #[test]
    fn color_adjustments_preserve_alpha_and_compositing_is_separate() {
        let image = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 128]));
        let s = WallpaperSettings {
            saturation: 0.,
            opacity: 0.,
            overlay: 1.,
            ..Default::default()
        };
        let output = process(DynamicImage::ImageRgba8(image), &s);
        let p = output.get_pixel(0, 0);
        assert_eq!(p[0], p[1]);
        assert_eq!(p[1], p[2]);
        assert_eq!(p[3], 128);
    }
    #[test]
    fn import_is_atomic_deduplicated_and_unmodified() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("input.png");
        colored().save(&source).unwrap();
        let managed = directory.path().join("managed");
        let first = import_asset(&source, &managed).unwrap();
        let second = import_asset(&source, &managed).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read(&first).unwrap(), fs::read(&source).unwrap());
        assert_eq!(fs::read_dir(&managed).unwrap().count(), 1);
        assert_eq!(
            resolve_asset(&managed, first.file_name().unwrap().to_str().unwrap()).unwrap(),
            first
        );
        assert!(resolve_asset(&managed, "../input.png").is_err());
        fs::write(&first, b"corrupt").unwrap();
        assert!(import_asset(&source, &managed).is_err());
    }
    #[test]
    fn rejects_missing_corrupt_unsupported_and_oversized() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.png");
        assert!(load_processed(&path, &WallpaperSettings::default()).is_err());
        fs::write(&path, b"not an image").unwrap();
        assert!(load_processed(&path, &WallpaperSettings::default()).is_err());
        assert!(decode(b"GIF89a").is_err());
        File::create(&path)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(read_bounded(&path).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_managed_symlink() {
        let directory = tempfile::tempdir().unwrap();
        let name = format!("{}.png", "a".repeat(64));
        let target = directory.path().join("original");
        fs::write(&target, b"image").unwrap();
        std::os::unix::fs::symlink(&target, directory.path().join(&name)).unwrap();
        assert!(resolve_asset(directory.path(), &name).is_err());
    }
    #[test]
    fn exif_orientation_precedes_crop() {
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(colored())
            .write_to(&mut encoded, ImageFormat::Jpeg)
            .unwrap();
        // APP1 Exif: little-endian TIFF, one SHORT orientation entry, value 6.
        let exif: &[u8] =
            b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let jpeg = encoded.into_inner();
        let mut oriented = jpeg[..2].to_vec();
        oriented.extend_from_slice(&[0xff, 0xe1]);
        oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        oriented.extend_from_slice(exif);
        oriented.extend_from_slice(&jpeg[2..]);
        let source = decode(&oriented).unwrap();
        assert_eq!((source.width(), source.height()), (2, 3));
        let s = WallpaperSettings {
            crop: [0., 0., 0.5, 1.],
            ..Default::default()
        };
        assert_eq!(process(source, &s).dimensions(), (1, 3));
    }
    #[test]
    fn all_supported_codecs_decode() {
        for format in [
            ImageFormat::Png,
            ImageFormat::Jpeg,
            ImageFormat::WebP,
            ImageFormat::Bmp,
        ] {
            let mut bytes = Cursor::new(Vec::new());
            DynamicImage::ImageRgba8(colored())
                .write_to(&mut bytes, format)
                .unwrap();
            let decoded = decode(bytes.get_ref()).unwrap();
            assert_eq!((decoded.width(), decoded.height()), (3, 2));
        }
    }
    #[test]
    fn no_filter_preserves_low_alpha_exactly() {
        let input = RgbaImage::from_pixel(1, 1, Rgba([19, 71, 241, 1]));
        assert_eq!(
            process(
                DynamicImage::ImageRgba8(input.clone()),
                &WallpaperSettings::default()
            ),
            input
        );
    }
    #[test]
    fn original_fit_uses_logical_dimensions_after_bounded_processing() {
        let source = RgbaImage::from_pixel(8192, 2, Rgba([255, 0, 0, 255]));
        let s = WallpaperSettings {
            fit: Fit::Original,
            rotation: 90.,
            crop: [0., 0., 0.75, 1.],
            ..Default::default()
        };
        let logical = transformed_size(source.dimensions(), &s);
        let pixels = process(DynamicImage::ImageRgba8(source), &s);
        assert_eq!(logical, (2, 6144));
        assert!(pixels.height() <= 4096);
        let layout = geometry(logical, (800., 600.), &s);
        assert_eq!((layout.width, layout.height), (2., 6144.));
    }
    #[test]
    fn alpha_blur_does_not_leak_hidden_rgb() {
        let mut input = RgbaImage::from_pixel(3, 1, Rgba([0, 0, 255, 0]));
        input.put_pixel(1, 0, Rgba([255, 0, 0, 255]));
        let s = WallpaperSettings {
            blur: 1.,
            ..Default::default()
        };
        let output = process(DynamicImage::ImageRgba8(input), &s);
        assert!(output.pixels().all(|p| p[2] == 0));
        assert!(output.pixels().filter(|p| p[3] > 0).all(|p| p[0] > 250));
    }
    #[test]
    fn huge_bmp_dimensions_are_rejected_before_pixel_allocation() {
        let mut bytes = vec![0u8; 54];
        bytes[..2].copy_from_slice(b"BM");
        bytes[2..6].copy_from_slice(&54u32.to_le_bytes());
        bytes[10..14].copy_from_slice(&54u32.to_le_bytes());
        bytes[14..18].copy_from_slice(&40u32.to_le_bytes());
        bytes[18..22].copy_from_slice(&100_000u32.to_le_bytes());
        bytes[22..26].copy_from_slice(&100_000u32.to_le_bytes());
        bytes[26..28].copy_from_slice(&1u16.to_le_bytes());
        bytes[28..30].copy_from_slice(&24u16.to_le_bytes());
        assert!(decode(&bytes).is_err());
    }
    #[test]
    fn rejects_webp_chunk_bombs_before_metadata_decode() {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend_from_slice(b"WEBPEXIF");
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(validate_webp(&bytes).is_err());
        assert!(decode(&bytes).is_err());
        let animated = b"RIFF\x0c\0\0\0WEBPANIM\0\0\0\0";
        assert!(validate_webp(animated).is_err());
        let truncated = b"RIFF\x06\0\0\0WEBPxx";
        assert!(validate_webp(truncated).is_err());
    }
    #[test]
    fn rejects_webp_inner_dimension_bombs_and_mismatches() {
        fn container(width: u16, height: u16) -> Vec<u8> {
            let mut bytes = b"RIFF\x28\0\0\0WEBPVP8X\x0a\0\0\0".to_vec();
            bytes.extend_from_slice(&[0; 10]); // 1 x 1 outer canvas
            bytes.extend_from_slice(b"VP8 \x0a\0\0\0\0\0\0\x9d\x01\x2a");
            bytes.extend_from_slice(&width.to_le_bytes());
            bytes.extend_from_slice(&height.to_le_bytes());
            bytes
        }
        assert!(validate_webp(&container(16383, 16383)).is_err());
        assert!(validate_webp(&container(2, 2)).is_err());
        assert!(validate_webp(&container(1, 1)).is_ok());
    }
}
