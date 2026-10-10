//! 三平台真实 UI / OS 凭据集成测试；main 保证 AppKit / GTK 主线程。
//! 仅显式 desktop-session 特性执行，合成值不输出；经根 test:native 运行。
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use aoidos_llm::credentials::{CredentialStore, OsKeyring};
    use secrecy::{ExposeSecret, SecretString};
    aoidos_llm::platform::verify_native_input()?;
    let store = OsKeyring::new();
    let id = format!("native-fixture-{}", std::process::id());
    store.set_key(&id, SecretString::from("aoidos-os-fixture".to_owned()))?;
    let result = store.get_key(&id);
    let clear = store.clear_key(&id);
    let result = result?;
    clear?;
    let matched = result
        .as_ref()
        .is_some_and(|secret| secret.expose_secret() == "aoidos-os-fixture");
    assert!(matched && store.get_key(&id)?.is_none());
    verify_locale_surfaces()?;
    println!("native confirm/cancel, OS credentials, locale menu/tray/title: PASS");
    Ok(())
}

fn verify_locale_surfaces() -> Result<(), Box<dyn std::error::Error>> {
    use aoidos_lib::locale_commands::{LocaleService, NativeStatus, apply_native};
    use aoidos_locale::preference::Locale;
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let context = tauri::generate_context!("tauri.conf.json");
    let app = tauri::Builder::default()
        .setup(|app| {
            let service = LocaleService::install(app, Locale::En)?;
            app.manage(service);
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Aoidos")
                .initialization_script(
                    "window.__AOIDOS_LOCALE_BOOTSTRAP__={version:1,locale:'zh-Hans'};",
                )
                .build()?;
            let window = app.get_webview_window("main").unwrap();
            window.set_title("stale fixture title")?;
            let status = apply_native(app.handle(), Locale::ZhHans);
            assert_eq!(status, NativeStatus::Applied);
            assert_locale_menu(app, "显示 Aoidos", "退出 Aoidos")?;
            assert_eq!(window.title()?, "Aoidos");
            window.set_title("stale fixture title")?;
            let status = apply_native(app.handle(), Locale::En);
            assert_eq!(status, NativeStatus::Applied);
            assert_locale_menu(app, "Show Aoidos", "Quit Aoidos")?;
            assert_eq!(window.title()?, "Aoidos");
            let handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(500));
                handle.exit(0);
            });
            Ok(())
        })
        .build(context)?;
    app.run(|_, _| {});
    Ok(())
}

fn assert_locale_menu(
    app: &tauri::App<tauri::Wry>,
    expected_show: &str,
    expected_quit: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use tauri::Manager;

    let menu = app
        .state::<aoidos_lib::locale_commands::LocaleService>()
        .window_menu();
    let root_items = menu.items()?;
    assert_eq!(root_items.len(), 1);
    let submenu = root_items[0]
        .as_submenu()
        .ok_or("window menu item is not a submenu")?;
    let items = submenu.items()?;
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[0]
            .as_menuitem()
            .ok_or("show entry is not a menu item")?
            .text()?,
        expected_show
    );
    assert_eq!(
        items[1]
            .as_menuitem()
            .ok_or("quit entry is not a menu item")?
            .text()?,
        expected_quit
    );
    Ok(())
}
