//! GTK 原生密码输入；无 DISPLAY / Wayland 会话时明确失败，不转 Webview。
use super::NativePromptText;
use aoidos_store::error::{Result, StoreError};
use gtk::prelude::*;

pub(super) fn prompt_native_key(label: &str, text: &NativePromptText) -> Result<Option<String>> {
    prompt(label, text, None)
}

fn prompt(
    label: &str,
    text: &NativePromptText,
    verification: Option<bool>,
) -> Result<Option<String>> {
    gtk::init().map_err(init_error)?;
    let dialog = gtk::Dialog::with_buttons(
        Some(&text.title),
        None::<&gtk::Window>,
        gtk::DialogFlags::MODAL,
        &[
            (&text.cancel, gtk::ResponseType::Cancel),
            (&text.save, gtk::ResponseType::Accept),
        ],
    );
    let entry = gtk::Entry::new();
    entry.set_visibility(false);
    entry.set_input_purpose(gtk::InputPurpose::Password);
    entry.set_activates_default(true);
    dialog.set_default_response(gtk::ResponseType::Accept);
    let message = text.message.replace("{provider}", label);
    dialog.content_area().add(&gtk::Label::new(Some(&message)));
    dialog.content_area().add(&entry);
    dialog.show_all();
    #[cfg(feature = "native-smoke")]
    if let Some(accept) = verification {
        entry.set_text("aoidos-native-fixture");
        let cloned = dialog.clone();
        gtk::glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
            cloned.response(if accept {
                gtk::ResponseType::Accept
            } else {
                gtk::ResponseType::Cancel
            })
        });
    }
    #[cfg(not(feature = "native-smoke"))]
    let _ = verification;
    let response = dialog.run();
    let secret = if response == gtk::ResponseType::Accept {
        Some(entry.text().to_string())
    } else {
        None
    };
    entry.set_text("");
    dialog.close();
    Ok(secret)
}
fn init_error(_: gtk::glib::BoolError) -> StoreError {
    StoreError::Io {
        code: "io",
        source: std::io::Error::other("native input requires desktop session"),
    }
}

#[cfg(feature = "native-smoke")]
pub fn verify_native_input() -> Result<()> {
    let text = NativePromptText::default();
    if prompt("test fixture", &text, Some(true))?.as_deref() != Some("aoidos-native-fixture")
        || prompt("test fixture", &text, Some(false))?.is_some()
    {
        return Err(StoreError::Corrupt(
            "native input verification failed".into(),
        ));
    }
    Ok(())
}
