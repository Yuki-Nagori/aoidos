//! AppKit 原生密码输入。仅 UI 主线程可用；密码字段不使用普通文本框。
use mythos_store::error::{Result, StoreError};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSAlert, NSApplication, NSSecureTextField};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};

pub(super) fn prompt_native_key(label: &str) -> Result<Option<String>> {
    prompt(label, None)
}

fn prompt(label: &str, verification: Option<bool>) -> Result<Option<String>> {
    let mtm = MainThreadMarker::new().ok_or_else(wrong_thread)?;
    let app = NSApplication::sharedApplication(mtm);
    let alert = NSAlert::new(mtm);
    alert.setMessageText(&NSString::from_str("Mythos API 密钥"));
    alert.setInformativeText(&NSString::from_str(&format!(
        "输入 {label} 的 API 密钥；取消保留旧值。"
    )));
    alert.addButtonWithTitle(&NSString::from_str("保存"));
    alert.addButtonWithTitle(&NSString::from_str("取消"));
    let entry = NSSecureTextField::initWithFrame(
        NSSecureTextField::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(320.0, 26.0)),
    );
    alert.setAccessoryView(Some(&entry));
    #[cfg(feature = "native-smoke")]
    let _timer = verification.map(|accept| {
        use objc2_foundation::{NSRunLoop, NSTimer};
        entry.setStringValue(&NSString::from_str("mythos-native-fixture"));
        let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            app.stopModalWithCode(if accept { 1000 } else { 1001 })
        });
        // SAFETY: 回调与 timer 全部在主线程创建、主线程 modal loop 执行；
        // 回调持有 app，timer 生命周期覆盖 runModal，不接触用户真实密钥。
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.1, false, &callback) };
        // SAFETY: timer 和运行循环均属于当前主线程；只加入明确的 modal 模式。
        unsafe {
            NSRunLoop::mainRunLoop()
                .addTimer_forMode(&timer, &NSString::from_str("NSModalPanelRunLoopMode"));
        }
        timer
    });
    #[cfg(not(feature = "native-smoke"))]
    let _ = (app, verification);
    let response = alert.runModal();
    let secret = if response == 1000 {
        Some(entry.stringValue().to_string())
    } else {
        None
    };
    entry.setStringValue(&NSString::from_str(""));
    Ok(secret)
}

fn wrong_thread() -> StoreError {
    StoreError::Io {
        code: "io",
        source: std::io::Error::other("native input requires UI main thread"),
    }
}

#[cfg(feature = "native-smoke")]
pub fn verify_native_input() -> Result<()> {
    if prompt("test fixture", Some(true))?.as_deref() != Some("mythos-native-fixture")
        || prompt("test fixture", Some(false))?.is_some()
    {
        return Err(StoreError::Corrupt(
            "native input verification failed".into(),
        ));
    }
    Ok(())
}
