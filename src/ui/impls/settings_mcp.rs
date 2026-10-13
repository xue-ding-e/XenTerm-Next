//! Local MCP setup guidance. No profile or permission is changed by viewing or
//! copying it; only the executable and the selected profile path are included.

use super::*;

#[path = "settings_mcp_config.rs"]
mod config;
use config::{client_config, ConfigError};

pub(super) struct McpSetup {
    json: Result<String, ConfigError>,
    preview: Option<Entity<TextareaState>>,
    data_dir: std::path::PathBuf,
    explicit_profile: bool,
    copied: bool,
}

impl McpSetup {
    fn new(window: &mut Window, cx: &mut Context<SettingsView>) -> Self {
        let data_dir = crate::config::data_dir();
        let explicit_profile = crate::config::has_explicit_data_dir();
        let json = std::env::current_exe()
            .map_err(|_| ConfigError::ExecutableUnavailable)
            .and_then(|executable| {
                client_config(&executable, explicit_profile.then_some(data_dir.as_path()))
            });
        let preview = json.as_ref().ok().map(|json| {
            cx.new(|cx| {
                TextareaState::new(window, cx)
                    .rows(12)
                    .default_value(json.clone())
            })
        });
        Self {
            json,
            preview,
            data_dir,
            explicit_profile,
            copied: false,
        }
    }
}

pub(super) fn setup_item(
    view: &mut SettingsView,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> SettingItem {
    let setup = view
        .mcp_setup
        .get_or_insert_with(|| McpSetup::new(window, cx));
    let preview = setup.preview.clone();
    let json = setup.json.as_ref().ok().cloned();
    let copied = setup.copied;
    let explicit_profile = setup.explicit_profile;
    let profile_path = format!(
        "{}: {}",
        crate::i18n::t("当前 XenTerm 数据目录", "Current XenTerm data directory"),
        setup.data_dir.display()
    );
    let database_path = format!(
        "{}: {}",
        crate::i18n::t("会话与权限数据库", "Sessions and permissions database"),
        setup.data_dir.join("sessions.db").display()
    );
    let settings = cx.entity().downgrade();
    let field = SettingField::element(
        move |_: &gpui_kit::component::setting::RenderOptions,
              _: &mut Window,
              _: &mut gpui_kit::App| {
            let settings = settings.clone();
            let copy_json = json.clone();
            v_flex()
                .w_full().min_w_0().gap_2()
                .when_some(preview.clone(), |field, preview| {
                    field.child(Textarea::new(&preview).readonly(true).h(px(300.)))
                })
                .when(json.is_none(), |field| field.child(div().text_sm().child(crate::i18n::t(
                    "无法生成配置：当前程序或数据目录不是可用的绝对 Unicode 路径。请确认安装路径后重启应用再试。",
                    "Cannot generate the config: the executable or data directory is not a usable absolute Unicode path. Check the installation path and restart the application.",
                ))))
                .child(
                    Button::new("settings-copy-mcp-config")
                        .debug_selector(|| "settings-copy-mcp-config".to_string())
                        .icon(IconName::Copy)
                        .label(if copied { crate::i18n::t("已复制 JSON", "JSON copied") } else { crate::i18n::t("复制 JSON 配置", "Copy JSON config") })
                        .outline().small().disabled(json.is_none())
                        .on_click(move |_, _, cx| {
                            let Some(json) = &copy_json else { return; };
                            cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string(json.clone()));
                            if let Some(settings) = settings.upgrade() {
                                settings.update(cx, |view, cx| {
                                    if let Some(setup) = &mut view.mcp_setup { setup.copied = true; }
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(div().text_sm().child(crate::i18n::t(
                    "1. 按需选择访问模式并启用 MCP 服务\n2. 在 MCP 客户端中打开 JSON 配置，将 xenterm 合入现有 mcpServers；配置文件位置与格式由客户端决定；其他格式请按客户端文档填写 command 和 args\n3. 保存后在客户端重连或重启 MCP 服务，再检查工具列表。移动或更新安装位置后重新复制本配置",
                    "1. Choose the access mode you need and enable MCP\n2. Open your MCP client's JSON config and merge xenterm into its existing mcpServers; the file location and format depend on the client; for other formats map command and args using that client’s documentation\n3. Save, reconnect or restart MCP in the client, then check the tool list. Copy this config again if the installation moves",
                )))
                .child(div().text_sm().child(profile_path.clone()))
                .child(div().text_sm().child(database_path.clone()))
                .child(div().text_sm().child(if explicit_profile {
                    crate::i18n::t(
                        "此进程使用独立数据目录，示例已在 mcp serve 前加入 --data-dir，保证客户端连接同一配置。",
                        "This process uses an explicit profile. The example puts --data-dir before mcp serve so the client opens the same profile.",
                    )
                } else {
                    crate::i18n::t(
                        "示例沿用桌面版默认数据目录和系统钥匙串。客户端不要设置 XENTERM_DATA_DIR 或 MEATSHELL_DATA_DIR，否则会选择其他独立配置。",
                        "The example uses the desktop default profile and keyring. Do not set XENTERM_DATA_DIR or MEATSHELL_DATA_DIR in the client environment: those select an explicit profile.",
                    )
                }))
                .child(div().text_sm().child(crate::i18n::t(
                    "以上数据库由 XenTerm 管理，请勿把客户端 JSON 写入 sessions.db。示例不含密码或私钥，也不会开启配置导入。HTTP 远程接入须另行配置认证、独立数据目录和 --http-config，本示例仅用于本机 stdio。",
                    "XenTerm manages this database; do not write client JSON into sessions.db. The example contains no passwords or private keys and does not enable config import. Remote HTTP requires separate authentication, an explicit profile and --http-config; this example is for local stdio only.",
                )))
                .into_any_element()
        },
    );
    SettingItem::new(crate::i18n::t("连接本机 MCP 客户端", "Connect a local MCP client"), field)
        // JSON needs the page width rather than the narrow settings control column.
        .layout(Axis::Vertical)
        .description(crate::i18n::t(
            "常见 mcpServers JSON 格式，command 已填入当前运行程序的真实路径。参数由客户端直接传递，无需额外 shell 引号。",
            "Common mcpServers JSON format with this running executable's actual path in command. The client passes arguments directly; no extra shell quoting is needed.",
        ))
}

#[cfg(test)]
#[path = "settings_mcp_tests.rs"]
mod tests;
