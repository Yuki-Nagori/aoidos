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
    println!("native confirm/cancel and OS credentials set/get/clear: PASS");
    Ok(())
}
