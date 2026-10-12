//! Desktop-only adapter. Decoding is background work, geometry stays toolkit-free,
//! and paint only consumes a cached image. No terminal text or shell protocol changes.
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};

use crate::wallpaper::{Scope, WallpaperSettings};
use gpui_kit::{
    canvas, div, fill, point, prelude::*, px, size, App, Bounds, Context, Global, ImageId,
    IntoElement, Render, RenderImage, Task, Window,
};

/// One bounded decode/transform operation across previews and native imports.
pub(crate) static IMAGE_WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

thread_local! {
    // A small shared cache lets split panes reuse one GPU image. Entities retain
    // their active image; old editor variations do not accumulate indefinitely.
    static IMAGES: RefCell<VecDeque<(String, CachedImage)>> = const { RefCell::new(VecDeque::new()) };
}

/// CPU cache references do not own GPU atlas entries. GPUI does not release a
/// raw RenderImage's texture when its Arc is dropped, so track the live layers
/// separately and release an image only after its last pane/preview lets go.
#[derive(Default)]
struct ImageOwners(HashMap<ImageId, usize>);
impl Global for ImageOwners {}

fn retain_image(image: &RenderImage, cx: &mut App) {
    *cx.default_global::<ImageOwners>()
        .0
        .entry(image.id)
        .or_default() += 1;
}

fn release_image(image: Arc<RenderImage>, cx: &mut App) {
    let owners = &mut cx.default_global::<ImageOwners>().0;
    let count = owners.get_mut(&image.id).expect("wallpaper image owner");
    *count -= 1;
    if *count != 0 {
        return;
    }
    owners.remove(&image.id);
    // Settings and release callbacks may run while their window is borrowed out
    // of App. Defer until it is returned so drop_image reaches every window.
    // A cached image can be reacquired before then; preserve its live texture.
    cx.defer(move |cx| {
        if !cx.default_global::<ImageOwners>().0.contains_key(&image.id) {
            cx.drop_image(image, None);
        }
    });
}

#[derive(Clone)]
struct CachedImage {
    image: Arc<RenderImage>,
    logical_size: (u32, u32),
    repeats: (u32, u32),
}

pub(crate) struct WallpaperLayer {
    settings: WallpaperSettings,
    asset_dir: PathBuf,
    image: Option<Arc<RenderImage>>,
    dimensions: (u32, u32),
    repeats: (u32, u32),
    error: Option<String>,
    loading: bool,
    painted: bool,
    generation: u64,
    worker_generation: Arc<AtomicU64>,
    processing_key: String,
    task: Option<Task<()>>,
}

fn processing_key(settings: &WallpaperSettings) -> String {
    // Layout and compositing changes must not decode or blur the file again.
    let mut processing = settings.clone();
    if processing.fit != crate::wallpaper::Fit::Tile {
        processing.fit = crate::wallpaper::Fit::Cover;
    }
    processing.scope = Scope::TerminalPane;
    processing.anchor_x = 0.5;
    processing.anchor_y = 0.5;
    processing.offset_x = 0.;
    processing.offset_y = 0.;
    processing.zoom = 1.;
    processing.opacity = 1.;
    processing.overlay = 0.;
    serde_json::to_string(&processing).unwrap_or_default()
}

impl WallpaperLayer {
    pub(crate) fn new_in(
        settings: WallpaperSettings,
        asset_dir: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.on_release(|layer, cx| layer.replace_image(None, cx))
            .detach();
        let mut layer = Self {
            settings: WallpaperSettings::default(),
            asset_dir,
            image: None,
            dimensions: (0, 0),
            repeats: (1, 1),
            error: None,
            loading: false,
            painted: false,
            generation: 0,
            worker_generation: Arc::new(AtomicU64::new(0)),
            processing_key: String::new(),
            task: None,
        };
        layer.set_settings(settings, cx);
        layer
    }

    fn replace_image(&mut self, image: Option<Arc<RenderImage>>, cx: &mut App) {
        if let Some(image) = &image {
            retain_image(image, cx);
        }
        if let Some(previous) = std::mem::replace(&mut self.image, image) {
            release_image(previous, cx);
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }
    pub(crate) fn is_ready(&self) -> bool {
        self.image.is_some() && self.painted && !self.loading && self.error.is_none()
    }

    pub(crate) fn set_settings_in(
        &mut self,
        settings: WallpaperSettings,
        asset_dir: PathBuf,
        cx: &mut Context<Self>,
    ) {
        self.asset_dir = asset_dir;
        self.set_settings(settings, cx);
    }

    pub(crate) fn set_settings(&mut self, settings: WallpaperSettings, cx: &mut Context<Self>) {
        let settings = settings.normalized();
        let key = format!("{}:{}", self.asset_dir.display(), processing_key(&settings));
        let changed = key != self.processing_key;
        self.settings = settings;
        if !changed {
            cx.notify();
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.worker_generation.store(generation, Ordering::Release);
        self.processing_key = key.clone();
        self.task = None;
        self.error = None;
        self.replace_image(None, cx);
        self.loading = false;
        self.painted = false;
        if !self.settings.enabled || self.settings.source.is_empty() {
            cx.notify();
            return;
        }
        if let Some(image) = IMAGES.with(|cache| {
            cache
                .borrow()
                .iter()
                .find(|(k, _)| k == &key)
                .map(|(_, v)| v.clone())
        }) {
            self.dimensions = image.logical_size;
            self.repeats = image.repeats;
            self.replace_image(Some(image.image), cx);
            cx.notify();
            return;
        }
        let settings = self.settings.clone();
        let asset_dir = self.asset_dir.clone();
        let worker_generation = self.worker_generation.clone();
        let worker_key = key.clone();
        self.loading = true;
        self.task = Some(cx.spawn(async move |this, cx| {
            // Coalesce typing/slider bursts before any expensive work begins.
            cx.background_executor()
                .timer(std::time::Duration::from_millis(120))
                .await;
            // Wait asynchronously: many split panes must not occupy background
            // threads just waiting for the shared image memory budget.
            let Ok(permit) = IMAGE_WORKERS.acquire().await else {
                return;
            };
            let result = cx
                .background_executor()
                .spawn(async move {
                    // The running worker owns the permit even if its UI task is cancelled.
                    let _permit = permit;
                    anyhow::ensure!(
                        worker_generation.load(Ordering::Acquire) == generation,
                        "Superseded wallpaper preview"
                    );
                    // The gate also coalesces identical requests from split panes.
                    // All instances share this bounded cache and the GPU image id.
                    static PROCESSED: Mutex<VecDeque<(String, CachedImage)>> =
                        Mutex::new(VecDeque::new());
                    if let Some(cached) = PROCESSED
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .iter()
                        .find(|(key, _)| key == &worker_key)
                        .map(|(_, image)| image.clone())
                    {
                        return Ok(cached);
                    }
                    let path = crate::wallpaper::resolve_asset(&asset_dir, &settings.source)?;
                    let prepared = crate::wallpaper::load_prepared(&path, &settings)?;
                    let (mut pixels, repeats) = if settings.fit == crate::wallpaper::Fit::Tile {
                        repeat_small_texture(prepared.pixels)
                    } else {
                        (prepared.pixels, (1, 1))
                    };
                    // Conversion is CPU work too, and stays off the UI thread.
                    for pixel in pixels.pixels_mut() {
                        pixel.0.swap(0, 2);
                    }
                    let cached = CachedImage {
                        image: Arc::new(RenderImage::new([image::Frame::new(pixels)])),
                        logical_size: prepared.logical_size,
                        repeats,
                    };
                    let mut cache = PROCESSED.lock().unwrap_or_else(|e| e.into_inner());
                    cache.push_back((worker_key, cached.clone()));
                    while cache.len() > 2 {
                        cache.pop_front();
                    }
                    Ok::<_, anyhow::Error>(cached)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.loading = false;
                match result {
                    Ok(cached) => {
                        this.dimensions = cached.logical_size;
                        this.repeats = cached.repeats;
                        this.replace_image(Some(cached.image.clone()), cx);
                        IMAGES.with(|cache| {
                            let mut cache = cache.borrow_mut();
                            cache.retain(|(k, _)| k != &key);
                            cache.push_back((key, cached));
                            while cache.len() > 2 {
                                cache.pop_front();
                            }
                        });
                    }
                    Err(error) => {
                        this.error = Some(format!(
                            "{}: {error:#}",
                            crate::i18n::t("壁纸加载失败", "Wallpaper loading failed")
                        ));
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }
}

/// Turn a tiny image into an exactly repeating supertile before GPU upload.
/// The original logical period remains separate, so alignment never shifts.
fn repeat_small_texture(source: image::RgbaImage) -> (image::RgbaImage, (u32, u32)) {
    let (w, h) = source.dimensions();
    let mut rx = 1024_u32.div_ceil(w).max(1);
    let mut ry = 1024_u32.div_ceil(h).max(1);
    while u64::from(w) * u64::from(h) * u64::from(rx) * u64::from(ry) > 4 * 1024 * 1024 {
        if rx >= ry && rx > 1 {
            rx -= 1;
        } else if ry > 1 {
            ry -= 1;
        } else {
            break;
        }
    }
    if (rx, ry) == (1, 1) {
        return (source, (1, 1));
    }
    let repeated =
        image::RgbaImage::from_fn(w * rx, h * ry, |x, y| *source.get_pixel(x % w, y % h));
    (repeated, (rx, ry))
}

/// Paint only intersecting tiles, bounded independently of zoom and image size.
/// Small images have already been expanded into exact repeating supertiles.
fn tile_positions(start: f32, extent: f32, period: f32) -> Vec<f32> {
    let period = period.max(1.);
    let first = start.rem_euclid(period) - period;
    let count = ((extent - first) / period).ceil().max(0.) as usize;
    (0..count.min(4096))
        .map(|i| first + i as f32 * period)
        .collect()
}

impl Render for WallpaperLayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let generation = self.generation;
        let image = self.image.clone();
        let settings = self.settings.clone();
        let dimensions = self.dimensions;
        let repeats = self.repeats;
        let viewport = window.viewport_size();
        // Image opacity is independent of the dark readability overlay. Neither
        // changes the opacity of text, menus, dialogs, selections or the cursor.
        div()
            .size_full()
            .relative()
            .overflow_hidden()
            .child(
                div().absolute().inset_0().opacity(settings.opacity).child(
                    canvas(
                        |bounds, _, _| bounds,
                        move |bounds, _, window, cx| {
                            let Some(image) = image else {
                                return;
                            };
                            if !settings.enabled {
                                return;
                            }
                            let continuous = settings.scope == Scope::ContinuousTerminalCanvas;
                            let canvas_size = if continuous { viewport } else { bounds.size };
                            let origin = if continuous {
                                point(px(0.), px(0.))
                            } else {
                                bounds.origin
                            };
                            let layout = crate::wallpaper::geometry(
                                dimensions,
                                (f32::from(canvas_size.width), f32::from(canvas_size.height)),
                                &settings,
                            );
                            let tile_width = layout.width * repeats.0 as f32;
                            let tile_height = layout.height * repeats.1 as f32;
                            let draw = |x: f32, y: f32, window: &mut Window| {
                                let target = Bounds::new(
                                    origin + point(px(x), px(y)),
                                    size(px(tile_width), px(tile_height)),
                                );
                                window.paint_image(
                                    bounds,
                                    target,
                                    (0.).into(),
                                    image.clone(),
                                    0,
                                    false,
                                )
                            };
                            let mut failure = None;
                            if layout.tile {
                                let xs = tile_positions(
                                    layout.x,
                                    f32::from(canvas_size.width),
                                    tile_width,
                                );
                                let ys = tile_positions(
                                    layout.y,
                                    f32::from(canvas_size.height),
                                    tile_height,
                                );
                                // Tiny textures were repeated into a bounded supertile on
                                // the worker. Repetition preserves anchor phase and avoids
                                // changing Tile into Stretch under an extreme zoom.
                                if xs.len().saturating_mul(ys.len()) > 4096 {
                                    failure = Some(
                                        crate::i18n::t(
                                            "壁纸平铺超出渲染预算，请增大缩放倍数。",
                                            "Wallpaper tile layout exceeds the rendering budget; increase zoom.",
                                        )
                                        .into(),
                                    );
                                } else {
                                    'tiles: for y in ys {
                                        for &x in &xs {
                                            if let Err(error) = draw(x, y, window) {
                                                failure = Some(format!(
                                                    "{}: {error:#}",
                                                    crate::i18n::t("壁纸上传失败", "Wallpaper upload failed")
                                                ));
                                                break 'tiles;
                                            }
                                        }
                                    }
                                }
                            } else if let Err(error) = draw(layout.x, layout.y, window) {
                                failure = Some(format!(
                                    "{}: {error:#}",
                                    crate::i18n::t("壁纸上传失败", "Wallpaper upload failed")
                                ));
                            }
                            // A decoded image is not yet a usable GPU image. Make upload
                            // failure visible to the editor before Apply becomes available.
                            let _ = this.update(cx, |layer, cx| {
                                if layer.generation != generation {
                                    return;
                                }
                                let painted = failure.is_none();
                                if layer.painted != painted || layer.error != failure {
                                    layer.painted = painted;
                                    layer.error = failure;
                                    cx.notify();
                                }
                            });
                        },
                    )
                    .size_full(),
                ),
            )
            .child(
                canvas(|bounds, _, _| bounds, {
                    let overlay = if self.settings.enabled && self.image.is_some() {
                        self.settings.overlay
                    } else {
                        0.
                    };
                    move |bounds, _, window, _| {
                        window.paint_quad(fill(bounds, gpui_kit::black().opacity(overlay)));
                    }
                })
                .size_full(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::gpui::{TestAppContext, VisualTestContext};

    fn test_image() -> Arc<RenderImage> {
        Arc::new(RenderImage::new([image::Frame::new(
            image::RgbaImage::from_pixel(2, 2, image::Rgba([0, 0, 255, 255])),
        )]))
    }

    fn test_layer(
        image: Arc<RenderImage>,
        cx: &mut TestAppContext,
    ) -> gpui_kit::Entity<WallpaperLayer> {
        cx.new(|cx| {
            // Empty source avoids background decoding; this fixture supplies the
            // same RenderImage that a successful worker/cache lookup would use.
            let mut settings = WallpaperSettings::default();
            settings.enabled = true;
            let mut layer = WallpaperLayer::new_in(settings, PathBuf::new(), cx);
            layer.replace_image(Some(image), cx);
            layer
        })
    }

    fn paint_test_image(image: Arc<RenderImage>, cx: &mut VisualTestContext) {
        cx.draw(point(px(0.), px(0.)), size(px(20.), px(20.)), |_, _| {
            canvas(
                |bounds, _, _| bounds,
                move |bounds, _, window, _| {
                    window
                        .paint_image(bounds, bounds, (0.).into(), image, 0, false)
                        .unwrap();
                },
            )
            .size_full()
        });
    }

    #[gpui_kit::gpui::test]
    fn wallpaper_atlas_release_preserves_shared_panes_and_cleans_all_windows(
        cx: &mut TestAppContext,
    ) {
        let mut first_window = cx.add_empty_window().clone();
        let mut second_window = cx.add_empty_window().clone();
        // Keeping these Arcs also models pixels retained by the bounded caches.
        let original = test_image();
        let replacement = test_image();
        let first = test_layer(original.clone(), cx);
        let second = test_layer(original.clone(), cx);
        paint_test_image(original.clone(), &mut first_window);
        paint_test_image(original.clone(), &mut second_window);
        paint_test_image(replacement.clone(), &mut first_window);

        first_window.update(|_, cx| {
            first.update(cx, |layer, cx| {
                layer.replace_image(Some(replacement.clone()), cx)
            });
        });
        assert!(first_window.update(|window, _| window.has_image_atlas_entry(&original)));
        assert!(second_window.update(|window, _| window.has_image_atlas_entry(&original)));

        drop(second);
        cx.update(|_| {});
        cx.run_until_parked();
        assert!(!first_window.update(|window, _| window.has_image_atlas_entry(&original)));
        assert!(!second_window.update(|window, _| window.has_image_atlas_entry(&original)));
        assert!(first_window.update(|window, _| window.has_image_atlas_entry(&replacement)));

        drop(first);
        cx.update(|_| {});
        cx.run_until_parked();
        assert!(!first_window.update(|window, _| window.has_image_atlas_entry(&replacement)));
    }

    #[gpui_kit::gpui::test]
    fn wallpaper_disable_releases_gpu_texture_but_cached_pixels_can_be_reused(
        cx: &mut TestAppContext,
    ) {
        let mut window = cx.add_empty_window().clone();
        let cached = test_image();
        let layer = test_layer(cached.clone(), cx);
        paint_test_image(cached.clone(), &mut window);
        window.update(|_, cx| {
            layer.update(cx, |layer, cx| {
                layer.set_settings(WallpaperSettings::default(), cx)
            });
        });
        assert!(!window.update(|window, _| window.has_image_atlas_entry(&cached)));

        window.update(|_, cx| {
            layer.update(cx, |layer, cx| {
                layer.replace_image(Some(cached.clone()), cx)
            });
        });
        paint_test_image(cached.clone(), &mut window);
        assert!(window.update(|window, _| window.has_image_atlas_entry(&cached)));
        drop(layer);
        cx.update(|_| {});
        cx.run_until_parked();
        assert!(!window.update(|window, _| window.has_image_atlas_entry(&cached)));
    }

    #[gpui_kit::gpui::test]
    fn wallpaper_deferred_release_keeps_an_image_reacquired_in_the_same_update(
        cx: &mut TestAppContext,
    ) {
        let mut window = cx.add_empty_window().clone();
        let cached = test_image();
        let layer = test_layer(cached.clone(), cx);
        paint_test_image(cached.clone(), &mut window);
        window.update(|_, cx| {
            layer.update(cx, |layer, cx| {
                layer.replace_image(None, cx);
                layer.replace_image(Some(cached.clone()), cx);
            });
        });
        assert!(window.update(|window, _| window.has_image_atlas_entry(&cached)));
        drop(layer);
        cx.update(|_| {});
        cx.run_until_parked();
        assert!(!window.update(|window, _| window.has_image_atlas_entry(&cached)));
    }

    #[test]
    fn tiny_tile_supertile_preserves_exact_pattern_with_bounded_memory() {
        let source =
            image::RgbaImage::from_fn(2, 3, |x, y| image::Rgba([x as u8, y as u8, 9, 255]));
        let (pixels, repeats) = repeat_small_texture(source.clone());
        assert_eq!(pixels.dimensions(), (2 * repeats.0, 3 * repeats.1));
        assert!(u64::from(pixels.width()) * u64::from(pixels.height()) <= 4 * 1024 * 1024);
        for (x, y) in [(0, 0), (101, 37), (pixels.width() - 1, pixels.height() - 1)] {
            assert_eq!(pixels.get_pixel(x, y), source.get_pixel(x % 2, y % 3));
        }
    }
    #[test]
    fn layout_changes_share_processed_pixels() {
        let original = WallpaperSettings::default();
        let mut changed = original.clone();
        changed.zoom = 2.;
        changed.offset_x = 42.;
        changed.opacity = 0.2;
        changed.overlay = 0.8;
        changed.scope = Scope::ContinuousTerminalCanvas;
        assert_eq!(processing_key(&original), processing_key(&changed));
        changed.rotation = 90.;
        assert_ne!(processing_key(&original), processing_key(&changed));
    }
    #[test]
    fn tiling_handles_negative_offsets_without_unbounded_loops() {
        assert_eq!(tile_positions(-3., 10., 4.), vec![-3., 1., 5., 9.]);
        assert!(tile_positions(0., 100000., 0.).len() <= 4096);
    }
}
