//! Isolated wallpaper drafts. Preview shares the terminal renderer, while only
//! Apply persists; cancelling an import or a draft never changes live terminals.
use super::wallpaper_layer::WallpaperLayer;
use crate::{
    config::ConfigStore,
    wallpaper::{Fit, Scope, WallpaperSettings},
};
use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        input::{Input, InputEvent, InputState},
        switch::Switch,
        v_flex, ActiveTheme as _, Disableable as _, Selectable as _,
    },
    div,
    prelude::*,
    px, Context, Entity, IntoElement, Render, SharedString, Subscription, Window,
};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Number {
    AnchorX,
    AnchorY,
    OffsetX,
    OffsetY,
    Zoom,
    Rotation,
    CropX,
    CropY,
    CropWidth,
    CropHeight,
    Opacity,
    Overlay,
    Brightness,
    Saturation,
    Blur,
}
impl Number {
    const ALL: [Self; 15] = [
        Self::AnchorX,
        Self::AnchorY,
        Self::OffsetX,
        Self::OffsetY,
        Self::Zoom,
        Self::Rotation,
        Self::CropX,
        Self::CropY,
        Self::CropWidth,
        Self::CropHeight,
        Self::Opacity,
        Self::Overlay,
        Self::Brightness,
        Self::Saturation,
        Self::Blur,
    ];
    fn label(self) -> &'static str {
        use crate::i18n::t;
        match self {
            Self::AnchorX => t("水平锚点 (0–1)", "Horizontal anchor (0–1)"),
            Self::AnchorY => t("垂直锚点 (0–1)", "Vertical anchor (0–1)"),
            Self::OffsetX => t("水平偏移 (px)", "Horizontal offset (px)"),
            Self::OffsetY => t("垂直偏移 (px)", "Vertical offset (px)"),
            Self::Zoom => t("缩放 (0.1–10)", "Zoom (0.1–10)"),
            Self::Rotation => t("顺时针旋转 (−360–360°)", "Clockwise rotation (−360–360°)"),
            Self::CropX => t("裁剪左边 (0–1)", "Crop left (0–1)"),
            Self::CropY => t("裁剪上边 (0–1)", "Crop top (0–1)"),
            Self::CropWidth => t("裁剪宽度 (0–1)", "Crop width (0–1)"),
            Self::CropHeight => t("裁剪高度 (0–1)", "Crop height (0–1)"),
            Self::Opacity => t("图片不透明度 (0–1)", "Image opacity (0–1)"),
            Self::Overlay => t("黑色遮罩 (0–1)", "Dark overlay (0–1)"),
            Self::Brightness => t("亮度倍数 (0–2)", "Brightness (0–2)"),
            Self::Saturation => t("饱和度倍数 (0–2)", "Saturation (0–2)"),
            Self::Blur => t("模糊半径 (0–30)", "Blur radius (0–30)"),
        }
    }
    fn range(self) -> (f32, f32) {
        match self {
            Self::OffsetX | Self::OffsetY => (-100_000., 100_000.),
            Self::Zoom => (0.1, 10.),
            Self::Rotation => (-360., 360.),
            Self::Brightness | Self::Saturation => (0., 2.),
            Self::Blur => (0., 30.),
            _ => (0., 1.),
        }
    }
    fn get(self, s: &WallpaperSettings) -> f32 {
        match self {
            Self::AnchorX => s.anchor_x,
            Self::AnchorY => s.anchor_y,
            Self::OffsetX => s.offset_x,
            Self::OffsetY => s.offset_y,
            Self::Zoom => s.zoom,
            Self::Rotation => s.rotation,
            Self::CropX => s.crop[0],
            Self::CropY => s.crop[1],
            Self::CropWidth => s.crop[2],
            Self::CropHeight => s.crop[3],
            Self::Opacity => s.opacity,
            Self::Overlay => s.overlay,
            Self::Brightness => s.brightness,
            Self::Saturation => s.saturation,
            Self::Blur => s.blur,
        }
    }
    fn set(self, s: &mut WallpaperSettings, v: f32) {
        match self {
            Self::AnchorX => s.anchor_x = v,
            Self::AnchorY => s.anchor_y = v,
            Self::OffsetX => s.offset_x = v,
            Self::OffsetY => s.offset_y = v,
            Self::Zoom => s.zoom = v,
            Self::Rotation => s.rotation = v,
            Self::CropX => s.crop[0] = v,
            Self::CropY => s.crop[1] = v,
            Self::CropWidth => s.crop[2] = v,
            Self::CropHeight => s.crop[3] = v,
            Self::Opacity => s.opacity = v,
            Self::Overlay => s.overlay = v,
            Self::Brightness => s.brightness = v,
            Self::Saturation => s.saturation = v,
            Self::Blur => s.blur = v,
        }
    }
    fn parse(self, text: &str) -> Result<f32, String> {
        let (min, max) = self.range();
        text.trim()
            .parse::<f32>()
            .ok()
            .filter(|v| v.is_finite() && *v >= min && *v <= max)
            .ok_or_else(|| {
                format!(
                    "{}: {} {min}–{max}",
                    self.label(),
                    crate::i18n::t("请输入范围内的有限数值", "Enter a finite number in range")
                )
            })
    }
}
fn validate_crop(s: &WallpaperSettings) -> Result<(), String> {
    let [x, y, w, h] = s.crop;
    if w <= 0. || h <= 0. || x + w > 1.000001 || y + h > 1.000001 {
        Err(crate::i18n::t(
            "裁剪宽高必须大于 0，左边 + 宽度、上边 + 高度不能超过 1。",
            "Crop dimensions must be positive; left + width and top + height must not exceed 1.",
        )
        .into())
    } else {
        Ok(())
    }
}
struct NumericInput {
    kind: Number,
    input: Entity<InputState>,
    _subscription: Subscription,
}

pub(crate) struct WallpaperEditor {
    store: Rc<RefCell<ConfigStore>>,
    draft: WallpaperSettings,
    preview: Entity<WallpaperLayer>,
    fields: Vec<NumericInput>,
    error: Option<String>,
    invalid_fields: bool,
    status: Option<&'static str>,
    import_busy: bool,
    /// Cancel/reset invalidate outstanding imports, including a still-open picker.
    import_generation: u64,
    _preview_subscription: Subscription,
}
impl WallpaperEditor {
    pub(crate) fn new(
        store: Rc<RefCell<ConfigStore>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let draft = store.borrow().wallpaper_settings();
        let dir = store.borrow().wallpaper_asset_dir();
        let mut preview_settings = draft.clone();
        preview_settings.scope = Scope::TerminalPane;
        let preview = cx.new(|cx| WallpaperLayer::new_in(preview_settings, dir, cx));
        let subscription = cx.observe(&preview, |_, _, cx| cx.notify());
        let fields = Number::ALL
            .into_iter()
            .map(|kind| {
                let input = cx.new(|cx| {
                    InputState::new(window, cx).default_value(kind.get(&draft).to_string())
                });
                let subscription =
                    cx.subscribe_in(&input, window, |view, _, event: &InputEvent, _, cx| {
                        if matches!(
                            event,
                            InputEvent::Change | InputEvent::PressEnter { .. } | InputEvent::Blur
                        ) {
                            view.read_fields(cx);
                        }
                    });
                NumericInput {
                    kind,
                    input,
                    _subscription: subscription,
                }
            })
            .collect();
        Self {
            store,
            draft,
            preview,
            fields,
            error: None,
            invalid_fields: false,
            status: None,
            import_busy: false,
            import_generation: 0,
            _preview_subscription: subscription,
        }
    }
    fn sync_preview(&mut self, cx: &mut Context<Self>) {
        let mut settings = self.draft.clone();
        // This sample represents one complete canvas, even when the saved mode
        // aligns multiple terminal panes to their shared window coordinates.
        settings.scope = Scope::TerminalPane;
        self.preview
            .update(cx, |preview, cx| preview.set_settings(settings, cx));
        self.status = None;
        cx.notify();
    }
    fn read_fields(&mut self, cx: &mut Context<Self>) -> bool {
        // Parse all raw fields transactionally, so an invalid crop or partial number
        // cannot silently normalize into a different image or reach the profile.
        let mut next = self.draft.clone();
        for field in &self.fields {
            match field.kind.parse(&field.input.read(cx).value()) {
                Ok(value) => field.kind.set(&mut next, value),
                Err(error) => {
                    self.error = Some(error);
                    self.invalid_fields = true;
                    cx.notify();
                    return false;
                }
            }
        }
        if let Err(error) = validate_crop(&next) {
            self.error = Some(error);
            self.invalid_fields = true;
            cx.notify();
            return false;
        }
        self.error = None;
        self.invalid_fields = false;
        self.draft = next;
        self.sync_preview(cx);
        true
    }
    fn replace_draft(
        &mut self,
        draft: WallpaperSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.import_generation = self.import_generation.wrapping_add(1);
        self.import_busy = false;
        self.draft = draft;
        for field in &self.fields {
            let value = field.kind.get(&self.draft).to_string();
            field
                .input
                .update(cx, |input, cx| input.set_value(value, window, cx));
        }
        self.error = None;
        self.invalid_fields = false;
        self.sync_preview(cx);
    }
    fn import(&mut self, cx: &mut Context<Self>) {
        self.import_generation = self.import_generation.wrapping_add(1);
        let generation = self.import_generation;
        let dir = self.store.borrow().wallpaper_asset_dir();
        self.import_busy = true;
        self.error = None;
        self.status = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let picked = rfd::AsyncFileDialog::new()
                .add_filter(
                    "PNG / JPEG / WebP / BMP",
                    &["png", "jpg", "jpeg", "webp", "bmp"],
                )
                .pick_file()
                .await;
            let result = if let Some(file) = picked {
                let path = file.path().to_owned();
                // Decode validation and renderer transforms share a single memory
                // budget. Never hold this permit while the native picker is open.
                let permit = super::wallpaper_layer::IMAGE_WORKERS
                    .acquire()
                    .await
                    .expect("wallpaper worker gate stays open");
                Some(
                    cx.background_executor()
                        .spawn(async move {
                            let _permit = permit;
                            crate::wallpaper::import_asset(&path, &dir)
                        })
                        .await,
                )
            } else {
                None
            };
            let _ = this.update(cx, |view, cx| view.finish_import(generation, result, cx));
        })
        .detach();
    }
    fn finish_import(
        &mut self,
        generation: u64,
        result: Option<anyhow::Result<std::path::PathBuf>>,
        cx: &mut Context<Self>,
    ) {
        if self.import_generation != generation {
            return;
        }
        self.import_busy = false;
        match result {
            Some(Ok(path)) => {
                self.draft.source = path
                    .file_name()
                    .expect("managed asset basename")
                    .to_string_lossy()
                    .into_owned();
                self.draft.enabled = true;
                self.sync_preview(cx);
            }
            Some(Err(error)) => {
                self.error = Some(format!(
                    "{}: {error:#}",
                    crate::i18n::t("导入失败", "Import failed")
                ))
            }
            None => {}
        }
        cx.notify();
    }
    fn edit_numbers(
        &mut self,
        values: &[(Number, f32)],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Presets update only their own controls: unrelated incomplete raw input
        // remains visible and still prevents Apply.
        for &(kind, value) in values {
            let field = self
                .fields
                .iter()
                .find(|field| field.kind == kind)
                .expect("numeric control");
            field.input.update(cx, |input, cx| {
                input.set_value(value.to_string(), window, cx)
            });
        }
        self.read_fields(cx);
    }
    fn apply(&mut self, cx: &mut Context<Self>) {
        if self.import_busy || !self.read_fields(cx) {
            return;
        }
        if self.draft.enabled {
            if self.draft.source.is_empty() {
                self.error =
                    Some(crate::i18n::t("请先导入图片。", "Import an image first.").into());
                cx.notify();
                return;
            }
            let preview = self.preview.read(cx);
            if preview.is_loading() || !preview.is_ready() {
                self.error = Some(
                    preview
                        .error()
                        .unwrap_or(crate::i18n::t(
                            "请等待预览加载完成。",
                            "Wait for the preview to finish loading.",
                        ))
                        .to_string(),
                );
                cx.notify();
                return;
            }
        }
        let result = self
            .store
            .borrow_mut()
            .save_wallpaper_settings(self.draft.normalized());
        match result {
            Ok(()) => {
                self.error = None;
                self.status = Some(crate::i18n::t("已应用。", "Applied."));
                cx.refresh_windows();
            }
            Err(error) => {
                self.error = Some(format!(
                    "{}: {error:#}",
                    crate::i18n::t(
                        "保存失败，原设置已保留",
                        "Save failed; previous settings retained"
                    )
                ))
            }
        }
        cx.notify();
    }
}
impl Render for WallpaperEditor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use crate::i18n::t;
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let preview_error = self.preview.read(cx).error().map(str::to_owned);
        let loading = self.preview.read(cx).is_loading();
        let mut fits = h_flex().gap_2().flex_wrap();
        for (fit, label) in [
            (Fit::Cover, t("填满", "Cover")),
            (Fit::Contain, t("完整显示", "Contain")),
            (Fit::Stretch, t("拉伸", "Stretch")),
            (Fit::Original, t("原始大小", "Original")),
            (Fit::Tile, t("平铺", "Tile")),
        ] {
            fits = fits.child(
                Button::new(SharedString::from(format!("wallpaper-fit-{fit:?}")))
                    .label(label)
                    .selected(self.draft.fit == fit)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.draft.fit = fit;
                        view.sync_preview(cx);
                    })),
            );
        }
        let mut scopes = h_flex().gap_2().flex_wrap();
        for (scope, label) in [
            (Scope::TerminalPane, t("每个终端面板", "Each terminal pane")),
            (
                Scope::ContinuousTerminalCanvas,
                t("连续终端画布", "Continuous terminal canvas"),
            ),
        ] {
            scopes = scopes.child(
                Button::new(SharedString::from(format!("wallpaper-scope-{scope:?}")))
                    .label(label)
                    .selected(self.draft.scope == scope)
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.draft.scope = scope;
                        view.sync_preview(cx);
                    })),
            );
        }
        let mut anchors = v_flex().gap_1();
        for (row, vertical) in [0., 0.5, 1.].into_iter().enumerate() {
            let mut buttons = h_flex().gap_1().flex_wrap();
            for (column, horizontal) in [0., 0.5, 1.].into_iter().enumerate() {
                let labels = [
                    [
                        t("左上", "Top left"),
                        t("上中", "Top center"),
                        t("右上", "Top right"),
                    ],
                    [
                        t("左中", "Middle left"),
                        t("居中", "Center"),
                        t("右中", "Middle right"),
                    ],
                    [
                        t("左下", "Bottom left"),
                        t("下中", "Bottom center"),
                        t("右下", "Bottom right"),
                    ],
                ];
                let selector = format!("wallpaper-anchor-{row}-{column}");
                buttons = buttons.child(
                    Button::new(SharedString::from(selector.clone()))
                        .debug_selector(move || selector.clone())
                        .w(px(115.))
                        .label(labels[row][column])
                        .selected(
                            self.draft.anchor_x == horizontal && self.draft.anchor_y == vertical,
                        )
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.edit_numbers(
                                &[(Number::AnchorX, horizontal), (Number::AnchorY, vertical)],
                                window,
                                cx,
                            )
                        })),
                );
            }
            anchors = anchors.child(buttons);
        }
        let mut numbers = v_flex().gap_3();
        let sections: [(&str, &[Number]); 3] = [
            (
                t("位置与变换", "Position and transform"),
                &[
                    Number::AnchorX,
                    Number::AnchorY,
                    Number::OffsetX,
                    Number::OffsetY,
                    Number::Zoom,
                    Number::Rotation,
                ],
            ),
            (
                t("非破坏性裁剪", "Non-destructive crop"),
                &[
                    Number::CropX,
                    Number::CropY,
                    Number::CropWidth,
                    Number::CropHeight,
                ],
            ),
            (
                t("可读性与效果", "Readability and effects"),
                &[
                    Number::Opacity,
                    Number::Overlay,
                    Number::Brightness,
                    Number::Saturation,
                    Number::Blur,
                ],
            ),
        ];
        for (title, kinds) in sections {
            let mut row = h_flex().gap_3().flex_wrap().items_start();
            for field in self
                .fields
                .iter()
                .filter(|field| kinds.contains(&field.kind))
            {
                let label = field.kind.label();
                let selector = format!("wallpaper-number-{:?}", field.kind);
                row = row.child(
                    v_flex()
                        .debug_selector(move || selector.clone())
                        .w(px(190.))
                        .gap_1()
                        .child(div().text_sm().child(label))
                        .child(Input::new(&field.input).aria_label(label).w_full()),
                );
            }
            numbers = numbers
                .child(div().text_sm().child(title.to_string()))
                .child(row);
        }
        let source = if self.draft.source.is_empty() {
            t("尚未导入图片", "No image imported").to_string()
        } else {
            self.draft.source.clone()
        };
        v_flex().id("wallpaper-editor").debug_selector(||"wallpaper-editor".into()).w_full().h_full().overflow_y_scroll().p_4().gap_3()
            .child(div().text_lg().child(t("终端静态壁纸", "Static terminal wallpaper")))
            .child(div().text_sm().text_color(muted).child(t("修改仅在下方预览；应用后生效。工具栏与侧栏保持不透明。", "Changes stay in the preview until Apply. Toolbars and sidebars remain opaque.")))
            .child(Switch::new("wallpaper-enabled").label(t("启用壁纸", "Enable wallpaper")).checked(self.draft.enabled).on_click(cx.listener(|view,value:&bool,_,cx|{view.draft.enabled=*value;view.sync_preview(cx);})))
            .child(h_flex().gap_2().child(Button::new("wallpaper-import").debug_selector(||"wallpaper-import".into()).label(if self.import_busy {t("正在导入…","Importing…")} else {t("导入图片…","Import image…")}).disabled(self.import_busy).on_click(cx.listener(|view,_,_,cx|view.import(cx)))))
            .child(div().text_xs().text_color(muted).overflow_hidden().text_ellipsis().child(source))
            .child(div().text_xs().text_color(muted).child(t("PNG / JPEG / WebP / BMP，最大 32 MiB / 3200 万像素。原图保留，裁剪不破坏原图。", "PNG / JPEG / WebP / BMP, up to 32 MiB / 32 megapixels. Cropping preserves the imported original.")))
            .child(div().text_sm().child(t("单画布预览", "Single-canvas preview")))
            .child(div().relative().w_full().h(px(190.)).flex_shrink_0().overflow_hidden().rounded_md().bg(gpui_kit::rgb(0x101318))
                .child(self.preview.clone())
                .child(div().absolute().top(px(18.)).left(px(18.)).text_color(gpui_kit::rgb(0xe4e8ef)).child("user@host ~ $ ls\nDocuments  Downloads  Projects\nuser@host ~ $ ")))
            .when(loading,|this|this.child(div().text_sm().text_color(muted).child(t("正在生成预览…", "Rendering preview…"))))
            .when_some(preview_error,|this,error|this.child(div().text_sm().text_color(danger).child(error)))
            .child(div().text_sm().child(t("显示范围", "Scope"))).child(scopes)
            .child(div().text_xs().text_color(muted).child(t("连续模式按窗口坐标对齐所有终端面板；预览显示一张完整画布。", "Continuous mode aligns terminal panes in window coordinates; the preview shows one complete canvas.")))
            .child(div().text_sm().child(t("适配方式", "Fit"))).child(fits)
            .child(h_flex().gap_4().flex_wrap()
                .child(Switch::new("wallpaper-flip-x").label(t("水平翻转", "Flip horizontally")).checked(self.draft.flip_x).on_click(cx.listener(|view,value:&bool,_,cx|{view.draft.flip_x=*value;view.sync_preview(cx);})))
                .child(Switch::new("wallpaper-flip-y").label(t("垂直翻转", "Flip vertically")).checked(self.draft.flip_y).on_click(cx.listener(|view,value:&bool,_,cx|{view.draft.flip_y=*value;view.sync_preview(cx);}))))
            .child(h_flex().gap_2().flex_wrap()
                .child(Button::new("wallpaper-rotate-left").debug_selector(||"wallpaper-rotate-left".into()).label(t("左转 90°", "Rotate left 90°")).on_click(cx.listener(|view,_,window,cx|{ let rotation=(view.draft.rotation-90.).rem_euclid(360.);view.edit_numbers(&[(Number::Rotation,rotation)],window,cx); })))
                .child(Button::new("wallpaper-rotate-right").debug_selector(||"wallpaper-rotate-right".into()).label(t("右转 90°", "Rotate right 90°")).on_click(cx.listener(|view,_,window,cx|{ let rotation=(view.draft.rotation+90.).rem_euclid(360.);view.edit_numbers(&[(Number::Rotation,rotation)],window,cx); })))
                .child(Button::new("wallpaper-transform-reset").label(t("重置位置与变换", "Reset position and transform")).on_click(cx.listener(|view,_,window,cx|{view.draft.flip_x=false;view.draft.flip_y=false;view.edit_numbers(&[(Number::AnchorX,0.5),(Number::AnchorY,0.5),(Number::OffsetX,0.),(Number::OffsetY,0.),(Number::Zoom,1.),(Number::Rotation,0.)],window,cx);}))))
            .child(div().text_sm().child(t("锚点预设", "Anchor presets"))).child(anchors)
            .child(numbers)
            .child(div().text_xs().text_color(muted).child(t("锚点 0 / 0.5 / 1 对应左上 / 居中 / 右下；偏移范围 ±100000 px。裁剪坐标相对原图，宽高须大于 0 且不越界。", "Anchors 0 / 0.5 / 1 mean start / center / end; offsets accept ±100000 px. Crop coordinates are relative to the original and must define a positive rectangle inside it.")))
            .when_some(self.error.clone(),|this,error|this.child(div().text_sm().text_color(danger).child(error)))
            .when_some(self.status,|this,status|this.child(div().text_sm().child(status)))
            .child(h_flex().gap_2().flex_wrap()
                .child(Button::new("wallpaper-apply").debug_selector(||"wallpaper-apply".into()).primary().label(t("应用", "Apply")).disabled(self.import_busy || loading || self.invalid_fields).on_click(cx.listener(|view,_,_,cx|view.apply(cx))))
                .child(Button::new("wallpaper-cancel").debug_selector(||"wallpaper-cancel".into()).label(t("取消更改", "Cancel changes")).on_click(cx.listener(|view,_,window,cx|{let saved=view.store.borrow().wallpaper_settings();view.replace_draft(saved,window,cx);})))
                .child(Button::new("wallpaper-reset").debug_selector(||"wallpaper-reset".into()).label(t("恢复默认预览", "Reset draft to defaults")).on_click(cx.listener(|view,_,window,cx|view.replace_draft(WallpaperSettings::default(),window,cx)))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_drafts_reject_partial_nonfinite_and_out_of_range() {
        for raw in ["", "-", "abc", "NaN", "inf", "-inf", "1e99"] {
            assert!(Number::Zoom.parse(raw).is_err(), "{raw}");
        }
        assert!(Number::Zoom.parse("0.09").is_err());
        assert!(Number::Opacity.parse("1.1").is_err());
        assert!(Number::OffsetX.parse("100001").is_err());
        assert_eq!(Number::Zoom.parse(" 1.25 ").unwrap(), 1.25);
        assert_eq!(Number::OffsetY.parse("-100000").unwrap(), -100000.);
    }
    #[test]
    fn normalized_crop_is_nonempty_and_inside_original() {
        let mut draft = WallpaperSettings::default();
        assert!(validate_crop(&draft).is_ok());
        draft.crop = [0.5, 0.25, 0.5, 0.75];
        assert!(validate_crop(&draft).is_ok());
        draft.crop = [0.5, 0., 0.6, 1.];
        assert!(validate_crop(&draft).is_err());
        draft.crop = [0., 0., 0., 1.];
        assert!(validate_crop(&draft).is_err());
    }
    use gpui_kit::component::Root;
    use gpui_kit::gpui::{TestAppContext, VisualTestContext};

    struct Fixture {
        _directory: tempfile::TempDir,
        store: Rc<RefCell<ConfigStore>>,
    }
    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let cache = crate::config::ConfigFile::default();
            let store = Rc::new(RefCell::new(ConfigStore {
                path: directory.path().join("sessions.db"),
                backup_dir: None,
                saved_state: std::sync::Mutex::new(crate::config::SavedState::of_cache(&cache))
                    .into(),
                cache,
                key: [7; 32],
                keyring_enabled: false,
            }));
            Self {
                _directory: directory,
                store,
            }
        }
    }
    fn draw(cx: &mut VisualTestContext) {
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.simulate_next_frame(cx);
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
    }
    fn open<'a>(
        cx: &'a mut TestAppContext,
        fixture: &Fixture,
    ) -> (Entity<WallpaperEditor>, &'a mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let mut entity = None;
        let store = fixture.store.clone();
        let (_, cx) = cx.add_window_view(|window, cx| {
            let view = cx.new(|cx| WallpaperEditor::new(store, window, cx));
            entity = Some(view.clone());
            Root::new(view, window, cx)
        });
        cx.update(|window, _| {
            window.resize(gpui_kit::size(px(1100.), px(1800.)));
            window.activate_window();
        });
        draw(cx);
        (entity.unwrap(), cx)
    }
    fn change(view: &Entity<WallpaperEditor>, kind: Number, raw: &str, cx: &mut VisualTestContext) {
        let input = view.read_with(cx, |view, _| {
            view.fields
                .iter()
                .find(|field| std::mem::discriminant(&field.kind) == std::mem::discriminant(&kind))
                .unwrap()
                .input
                .clone()
        });
        cx.update(|window, cx| {
            input.update(cx, |input, cx| {
                input.set_value(raw, window, cx);
                cx.emit(InputEvent::Change);
            })
        });
        draw(cx);
    }
    fn click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        cx.simulate_click(bounds.center(), Default::default());
        draw(cx);
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_draft_apply_cancel_and_reset_are_isolated(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        change(&view, Number::Zoom, "2.5", cx);
        assert_eq!(view.read_with(cx, |view, _| view.draft.zoom), 2.5);
        assert_eq!(fixture.store.borrow().wallpaper_settings().zoom, 1.);
        assert!(!fixture.store.borrow().path.exists());
        click("wallpaper-apply", cx);
        assert_eq!(fixture.store.borrow().wallpaper_settings().zoom, 2.5);
        assert!(fixture.store.borrow().path.exists());
        let saved_bytes = std::fs::read(&fixture.store.borrow().path).unwrap();
        change(&view, Number::Zoom, "3", cx);
        click("wallpaper-cancel", cx);
        assert_eq!(view.read_with(cx, |view, _| view.draft.zoom), 2.5);
        click("wallpaper-reset", cx);
        assert_eq!(
            view.read_with(cx, |view, _| view.draft.clone()),
            WallpaperSettings::default()
        );
        assert_eq!(fixture.store.borrow().wallpaper_settings().zoom, 2.5);
        assert_eq!(
            std::fs::read(&fixture.store.borrow().path).unwrap(),
            saved_bytes
        );
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_invalid_raw_input_cannot_apply(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        for raw in ["-", "NaN", "99"] {
            change(&view, Number::Zoom, raw, cx);
            assert!(view.read_with(cx, |view, _| view.invalid_fields));
            click("wallpaper-apply", cx);
            assert!(!fixture.store.borrow().path.exists());
            assert_eq!(
                fixture.store.borrow().wallpaper_settings(),
                WallpaperSettings::default()
            );
        }
        change(&view, Number::Zoom, "1.5", cx);
        assert!(!view.read_with(cx, |view, _| view.invalid_fields));
        click("wallpaper-apply", cx);
        assert_eq!(fixture.store.borrow().wallpaper_settings().zoom, 1.5);
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_enabled_without_image_rejects_apply_and_cancel_invalidates_import(
        cx: &mut TestAppContext,
    ) {
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.draft.enabled = true;
                view.sync_preview(cx);
            })
        });
        draw(cx);
        click("wallpaper-apply", cx);
        assert!(view.read_with(cx, |view, _| view.error.is_some()));
        assert!(!fixture.store.borrow().path.exists());
        let generation = view.read_with(cx, |view, _| view.import_generation);
        click("wallpaper-cancel", cx);
        assert!(view.read_with(cx, |view, _| view.import_generation) > generation);
        assert!(!view.read_with(cx, |view, _| view.draft.enabled));
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_keyboard_edits_and_enter_never_persist_without_apply(cx: &mut TestAppContext) {
        use gpui_kit::Focusable as _;
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        let input = view.read_with(cx, |view, _| {
            view.fields
                .iter()
                .find(|f| f.kind == Number::Zoom)
                .unwrap()
                .input
                .clone()
        });
        cx.update(|window, cx| input.focus_handle(cx).focus(window, cx));
        draw(cx);
        let select_all = if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        };
        cx.simulate_keystrokes(select_all);
        cx.simulate_input("-");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert!(view.read_with(cx, |view, _| view.invalid_fields));
        assert!(!fixture.store.borrow().path.exists());
        cx.simulate_keystrokes(select_all);
        cx.simulate_input("1.75");
        cx.simulate_keystrokes("enter");
        draw(cx);
        assert_eq!(view.read_with(cx, |view, _| view.draft.zoom), 1.75);
        assert!(!fixture.store.borrow().path.exists());
        cx.simulate_keystrokes("tab");
        draw(cx);
        assert!(!fixture.store.borrow().path.exists());
        click("wallpaper-apply", cx);
        assert_eq!(fixture.store.borrow().wallpaper_settings().zoom, 1.75);
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_failed_save_preserves_live_settings_and_editable_draft(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        // A regular file cannot be the parent directory of the database.
        let blocker = fixture._directory.path().join("blocker");
        std::fs::write(&blocker, b"unchanged").unwrap();
        fixture.store.borrow_mut().path = blocker.join("sessions.db");
        let (view, cx) = open(cx, &fixture);
        change(&view, Number::Zoom, "2", cx);
        click("wallpaper-apply", cx);
        assert_eq!(
            fixture.store.borrow().wallpaper_settings(),
            WallpaperSettings::default()
        );
        assert_eq!(view.read_with(cx, |view, _| view.draft.zoom), 2.);
        assert!(view.read_with(cx, |view, _| view
            .error
            .as_deref()
            .unwrap()
            .contains("Save failed")
            || view.error.as_deref().unwrap().contains("保存失败")));
        assert_eq!(std::fs::read(blocker).unwrap(), b"unchanged");
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_cancel_ignores_late_background_import(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        let source = fixture._directory.path().join("source.png");
        image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]))
            .save(&source)
            .unwrap();
        let dir = fixture.store.borrow().wallpaper_asset_dir();
        let (release, wait) = futures::channel::oneshot::channel::<()>();
        let completed = Rc::new(std::cell::Cell::new(false));
        let completed_in_task = completed.clone();
        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.import_generation += 1;
                view.import_busy = true;
                let generation = view.import_generation;
                cx.spawn(async move |this, cx| {
                    wait.await.unwrap();
                    let result = cx
                        .background_executor()
                        .spawn(async move { crate::wallpaper::import_asset(&source, &dir) })
                        .await;
                    let _ = this.update(cx, |view, cx| {
                        view.finish_import(generation, Some(result), cx);
                        completed_in_task.set(true);
                    });
                })
                .detach();
                cx.notify();
            })
        });
        draw(cx);
        click("wallpaper-cancel", cx);
        release.send(()).unwrap();
        draw(cx);
        assert!(
            completed.get(),
            "late import must actually complete before checking the draft"
        );
        assert_eq!(
            view.read_with(cx, |view, _| view.draft.clone()),
            WallpaperSettings::default()
        );
        assert!(!view.read_with(cx, |view, _| view.import_busy));
        assert_eq!(
            fixture.store.borrow().wallpaper_settings(),
            WallpaperSettings::default()
        );
        assert!(!fixture.store.borrow().path.exists());
    }

    #[gpui_kit::gpui::test]
    fn wallpaper_anchor_and_rotation_presets_update_only_the_draft(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        click("wallpaper-anchor-2-0", cx);
        assert_eq!(
            view.read_with(cx, |view, _| (view.draft.anchor_x, view.draft.anchor_y)),
            (0., 1.)
        );
        click("wallpaper-rotate-left", cx);
        assert_eq!(view.read_with(cx, |view, _| view.draft.rotation), 270.);
        click("wallpaper-rotate-right", cx);
        assert_eq!(view.read_with(cx, |view, _| view.draft.rotation), 0.);
        assert!(!fixture.store.borrow().path.exists());
        assert_eq!(
            fixture.store.borrow().wallpaper_settings(),
            WallpaperSettings::default()
        );
    }
    fn settle_preview(cx: &mut VisualTestContext) {
        draw(cx);
        cx.background_executor
            .advance_clock(std::time::Duration::from_millis(250));
        draw(cx);
        draw(cx);
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_imported_image_applies_and_reopens_from_disk(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        let source = fixture._directory.path().join("source.png");
        image::RgbaImage::from_pixel(16, 8, image::Rgba([220, 30, 90, 255]))
            .save(&source)
            .unwrap();
        let imported =
            crate::wallpaper::import_asset(&source, &fixture.store.borrow().wallpaper_asset_dir())
                .unwrap();
        let (view, cx) = open(cx, &fixture);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let mut settings = WallpaperSettings::default();
                settings.enabled = true;
                settings.source = imported.file_name().unwrap().to_str().unwrap().into();
                settings.rotation = 90.;
                settings.crop = [0.25, 0., 0.5, 1.];
                settings.zoom = 1.75;
                view.replace_draft(settings, window, cx);
            })
        });
        settle_preview(cx);
        view.read_with(cx, |view, cx| {
            let preview = view.preview.read(cx);
            assert!(
                preview.is_ready(),
                "preview not ready: loading={}, error={:?}",
                preview.is_loading(),
                preview.error()
            );
        });
        assert!(!fixture.store.borrow().path.exists());
        click("wallpaper-apply", cx);
        assert!(fixture.store.borrow().wallpaper_settings().enabled);
        let connection = rusqlite::Connection::open(&fixture.store.borrow().path).unwrap();
        let raw: String = connection
            .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |row| {
                row.get(0)
            })
            .unwrap();
        let disk: crate::config::ConfigFile = serde_json::from_str(&raw).unwrap();
        let expected = fixture.store.borrow().wallpaper_settings();
        assert_eq!(disk.wallpaper_settings, expected);
        fixture.store.borrow_mut().cache = disk;
        let reopened = cx.update(|window, cx| {
            cx.new(|cx| WallpaperEditor::new(fixture.store.clone(), window, cx))
        });
        assert_eq!(
            reopened.read_with(cx, |view, _| view.draft.clone()),
            expected
        );
        assert!(imported.exists());
    }
    #[gpui_kit::gpui::test]
    fn wallpaper_missing_asset_finishes_with_error_and_never_applies(cx: &mut TestAppContext) {
        let fixture = Fixture::new();
        let (view, cx) = open(cx, &fixture);
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                let mut settings = WallpaperSettings::default();
                settings.enabled = true;
                settings.source = format!("{}.png", "a".repeat(64));
                view.replace_draft(settings, window, cx);
            })
        });
        settle_preview(cx);
        view.read_with(cx, |view, cx| {
            let preview = view.preview.read(cx);
            assert!(!preview.is_loading());
            assert!(!preview.is_ready());
            assert!(preview.error().is_some());
        });
        click("wallpaper-apply", cx);
        assert_eq!(
            fixture.store.borrow().wallpaper_settings(),
            WallpaperSettings::default()
        );
        assert!(!fixture.store.borrow().path.exists());
    }
}
