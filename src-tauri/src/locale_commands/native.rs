//! Wry 菜单、托盘和窗口标题适配；真实 OS 表面由 native-platform 集成测试验证。

use super::{NativeLocale, NativeStatus, native_message};
use aoidos_locale::preference::Locale;
use tauri::{
    Manager,
    menu::{Menu, MenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
};

pub(super) struct NativeSurfaces {
    app: tauri::AppHandle,
    window_menu: Menu<tauri::Wry>,
    window_submenu: Submenu<tauri::Wry>,
    window_show: MenuItem<tauri::Wry>,
    window_quit: MenuItem<tauri::Wry>,
    tray_show: MenuItem<tauri::Wry>,
    tray_quit: MenuItem<tauri::Wry>,
    tray: TrayIcon<tauri::Wry>,
}

impl NativeSurfaces {
    pub(super) fn install(app: &tauri::App, current: Locale) -> tauri::Result<Self> {
        let window_show = MenuItem::with_id(
            app,
            "locale-show",
            native_message(current, "menuShow"),
            true,
            None::<&str>,
        )?;
        let window_quit = MenuItem::with_id(
            app,
            "locale-quit",
            native_message(current, "menuQuit"),
            true,
            None::<&str>,
        )?;
        let window_submenu = Submenu::with_items(
            app,
            native_message(current, "windowTitle"),
            true,
            &[&window_show, &window_quit],
        )?;
        let window_menu = Menu::with_items(app, &[&window_submenu])?;
        app.set_menu(window_menu.clone())?;

        let tray_show = MenuItem::with_id(
            app,
            "locale-show",
            native_message(current, "menuShow"),
            true,
            None::<&str>,
        )?;
        let tray_quit = MenuItem::with_id(
            app,
            "locale-quit",
            native_message(current, "menuQuit"),
            true,
            None::<&str>,
        )?;
        let tray_menu = Menu::with_items(app, &[&tray_show, &tray_quit])?;
        let icon = app
            .default_window_icon()
            .expect("tauri.conf.json configures the application icon")
            .clone();
        let tray = TrayIconBuilder::with_id("aoidos")
            .icon(icon)
            .tooltip(native_message(current, "windowTitle"))
            .menu(&tray_menu)
            .on_menu_event(handle_menu_event)
            .build(app)?;

        Ok(Self {
            app: app.handle().clone(),
            window_menu,
            window_submenu,
            window_show,
            window_quit,
            tray_show,
            tray_quit,
            tray,
        })
    }
}

impl super::LocaleService {
    pub fn install(app: &tauri::App, current: Locale) -> tauri::Result<Self> {
        Ok(Self::new(
            current,
            Box::new(NativeSurfaces::install(app, current)?),
        ))
    }
}

impl NativeLocale for NativeSurfaces {
    fn window_menu(&self) -> Menu<tauri::Wry> {
        self.window_menu.clone()
    }

    fn apply(&self, locale: Locale) -> NativeStatus {
        let title = native_message(locale, "windowTitle");
        let show_text = native_message(locale, "menuShow");
        let quit_text = native_message(locale, "menuQuit");
        let title_text = native_message(locale, "windowTitle");
        let show_updated = self.window_show.set_text(&show_text);
        let quit_updated = self.window_quit.set_text(&quit_text);
        let submenu_updated = self.window_submenu.set_text(&title_text);
        let tray_show_updated = self.tray_show.set_text(&show_text);
        let tray_quit_updated = self.tray_quit.set_text(&quit_text);
        let tray_title_updated = self.tray.set_tooltip(Some(&title_text));
        let window_updated = self
            .app
            .get_webview_window("main")
            .is_some_and(|window| window.set_title(&title).is_ok());
        super::native_status([
            show_updated.is_ok(),
            quit_updated.is_ok(),
            submenu_updated.is_ok(),
            tray_show_updated.is_ok(),
            tray_quit_updated.is_ok(),
            tray_title_updated.is_ok(),
            window_updated,
        ])
    }
}

pub fn apply_native(app: &tauri::AppHandle, locale: Locale) -> NativeStatus {
    app.state::<super::LocaleService>().native.apply(locale)
}

pub fn handle_menu_event(app: &tauri::AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().as_ref() {
        "locale-show" => {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        "locale-quit" => app.exit(0),
        _ => {}
    }
}
