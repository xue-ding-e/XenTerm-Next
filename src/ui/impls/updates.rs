use std::{cell::RefCell, path::PathBuf, rc::Rc};

use gpui_kit::{
    component::{
        button::{Button, ButtonVariants},
        h_flex,
        switch::Switch,
        v_flex, ActiveTheme as _, Disableable as _,
    },
    div,
    gpui::Global,
    prelude::*,
    App, Context, Entity, Render, Window,
};

use crate::{
    app::updates::{self, Check, Download, Install, Update},
    config::ConfigStore,
    i18n::t,
};

struct DesktopUpdates(Entity<UpdateView>);
impl Global for DesktopUpdates {}

pub(super) fn init(store: Rc<RefCell<ConfigStore>>, cx: &mut App) {
    let automatically = store.borrow().update_check_enabled();
    let updater = cx.new(|_| UpdateView {
        store,
        status: Status::Idle,
        offered: None,
    });
    cx.set_global(DesktopUpdates(updater.clone()));
    if automatically {
        updater.update(cx, |view, cx| view.check(cx));
    }
}

pub(super) fn view(cx: &App) -> Option<Entity<UpdateView>> {
    cx.try_global::<DesktopUpdates>()
        .map(|global| global.0.clone())
}

enum Status {
    Idle,
    Checking,
    Current,
    Unavailable,
    CheckFailed,
    Available,
    Downloading,
    DownloadFailed,
    Saved(PathBuf),
    #[cfg(windows)]
    InstallerStarted,
    #[cfg(windows)]
    RestartToApply,
}

pub(super) struct UpdateView {
    store: Rc<RefCell<ConfigStore>>,
    status: Status,
    offered: Option<Box<Update>>,
}

impl UpdateView {
    pub(super) fn available_version(&self) -> Option<&str> {
        matches!(self.status, Status::Available | Status::DownloadFailed)
            .then(|| self.offered.as_deref().map(|offer| offer.version.as_str()))
            .flatten()
    }

    fn busy(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Downloading)
    }

    fn check(&mut self, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.status = Status::Checking;
        self.offered = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let checked = cx
                .background_executor()
                .spawn(async { updates::check() })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.status = match checked {
                    Ok(Check::Available(offer)) => {
                        view.offered = Some(offer);
                        Status::Available
                    }
                    Ok(Check::Current) => Status::Current,
                    Ok(Check::Unavailable) => Status::Unavailable,
                    Err(err) => {
                        tracing::debug!("GPUI update check failed: {err:#}");
                        Status::CheckFailed
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        if self.available_version().is_none() {
            return;
        }
        let Some(update) = self.offered.clone() else {
            return;
        };
        self.status = Status::Downloading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let directory = if matches!(update.install, Install::Archive) {
                let picked = rfd::AsyncFileDialog::new().pick_folder().await;
                let Some(picked) = picked else {
                    let _ = this.update(cx, |view, cx| {
                        view.status = Status::Available;
                        cx.notify();
                    });
                    return;
                };
                Some(picked.path().to_path_buf())
            } else {
                None
            };
            let result = cx
                .background_executor()
                .spawn(async move { update.download(directory.as_deref()) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.status = match result {
                    Ok(Download::Saved(path)) => Status::Saved(path),
                    #[cfg(windows)]
                    Ok(Download::InstallerStarted) => Status::InstallerStarted,
                    #[cfg(windows)]
                    Ok(Download::RestartToApply) => Status::RestartToApply,
                    Err(err) => {
                        tracing::warn!("GPUI update download failed: {err:#}");
                        Status::DownloadFailed
                    }
                };
                cx.notify();
            });
        })
        .detach();
    }

    fn status_text(&self) -> String {
        let message = match &self.status {
            Status::Idle => "",
            Status::Checking => t("正在检查 GPUI 更新…", "Checking for GPUI updates…"),
            Status::Current => t("当前没有更新的 GPUI 版本。", "No newer GPUI build is available."),
            Status::Unavailable => t("当前版本或平台没有 GPUI Beta 更新渠道。", "GPUI Beta updates are unavailable for this build or platform."),
            Status::CheckFailed => t("检查失败，请确认 GitHub 可访问后重试。", "Couldn't check for updates. Check access to GitHub and try again."),
            Status::Available => {
                return format!("{} {}", t("发现新版", "New build"), self.available_version().unwrap_or_default());
            }
            Status::Downloading => t("正在准备并校验更新包…", "Preparing and verifying the update…"),
            Status::DownloadFailed => t("无法准备更新包，请重试。", "Couldn't prepare the update. Please try again."),
            Status::Saved(path) => {
                return format!("{} {}", t("已校验并保存到", "Verified and saved to"), path.display());
            }
            #[cfg(windows)]
            Status::InstallerStarted => t("已打开 GPUI 安装程序，请按 Windows 安装提示继续。", "The GPUI installer is open. Follow the Windows Installer prompts."),
            #[cfg(windows)]
            Status::RestartToApply => t(
                "更新已就绪。退出当前 GPUI 和使用此程序的 MCP 进程，再从桌面快捷方式启动即可应用。",
                "Update ready. Close GPUI and MCP processes using this executable, then open the desktop shortcut to apply it.",
            ),
        };
        message.to_owned()
    }
}

impl Render for UpdateView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let install = self
            .offered
            .as_ref()
            .filter(|_| self.available_version().is_some())
            .map(|offer| match offer.install {
                Install::Archive => t("下载更新包", "Download update"),
                #[cfg(windows)]
                Install::Msi => t("下载并安装", "Download and install"),
                #[cfg(windows)]
                Install::ManagedPortable(_) => t("下载并准备更新", "Download and stage update"),
            });
        let automatic = self.store.borrow().update_check_enabled();
        v_flex()
            .gap_3()
            .child(
                Switch::new("gpui-auto-check-updates")
                    .label(t("启动时检查更新", "Check for updates on startup"))
                    .checked(automatic)
                    .on_click(cx.listener(|view, enabled: &bool, _, cx| {
                        {
                            let mut store = view.store.borrow_mut();
                            store.set_update_check_enabled(*enabled);
                            if let Err(err) = store.save() {
                                tracing::warn!(
                                    "could not save the GPUI update preference: {err:#}"
                                );
                            }
                        }
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .when(cfg!(test), |row| {
                        row.debug_selector(|| "gpui-update-actions".into())
                    })
                    .gap_2()
                    .child(
                        Button::new("gpui-check-updates")
                            .label(t("检查更新", "Check for updates"))
                            .disabled(self.busy())
                            .on_click(cx.listener(|view, _, _, cx| view.check(cx))),
                    )
                    .children(install.map(|label| {
                        Button::new("gpui-download-update")
                            .label(label)
                            .primary()
                            .on_click(cx.listener(|view, _, _, cx| view.download(cx)))
                    })),
            )
            .when(!matches!(self.status, Status::Idle), |section| {
                section.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.status_text()),
                )
            })
            .children(match &self.status {
                Status::Saved(path) => {
                    let path = path.clone();
                    Some(
                        Button::new("gpui-reveal-update")
                            .label(t("打开所在位置", "Show in folder"))
                            .on_click(move |_, _, cx| cx.reveal_path(&path)),
                    )
                }
                _ => None,
            })
    }
}
