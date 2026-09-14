//! Styled credits inside the standard macOS About panel, shared by both menus.
#[cfg(target_os = "macos")]
pub fn show(app: &tauri::AppHandle) {
    use super::native_message::{NativeMessageKey, native_message};
    use crate::core::locale::LocaleService;
    use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
    use objc2_app_kit::{
        NSAboutPanelOptionApplicationName, NSAboutPanelOptionApplicationVersion,
        NSAboutPanelOptionCredits, NSAboutPanelOptionVersion, NSApplication, NSColor, NSFont,
        NSFontAttributeName, NSForegroundColorAttributeName, NSLinkAttributeName,
        NSMutableParagraphStyle, NSParagraphStyleAttributeName, NSTextAlignment,
    };
    use objc2_foundation::{NSDictionary, NSMutableAttributedString, NSRange, NSString};
    use std::sync::Arc;
    use tauri::Manager;

    let owner = app.clone();
    let _ = app.run_on_main_thread(move || {
        let mtm = MainThreadMarker::new().expect("Tauri main thread");
        let package = owner.package_info();
        let locale = owner
            .state::<Arc<LocaleService>>()
            .snapshot()
            .effective_locale;
        let address = "github.com/RookieZoe/skill-man";
        let copy = native_message(
            locale,
            NativeMessageKey::AboutCredits,
            &[("author", package.authors), ("url", address)],
        );
        let paragraph = NSMutableParagraphStyle::new();
        paragraph.setAlignment(NSTextAlignment::Center);
        paragraph.setParagraphSpacing(5.0);
        let font = NSFont::systemFontOfSize(11.0);
        let color = NSColor::secondaryLabelColor();
        // SAFETY: AppKit attribute keys and values have the documented Cocoa types;
        // all objects are created and consumed on the main thread.
        unsafe {
            let attributes = NSDictionary::from_retained_objects(
                &[
                    NSFontAttributeName,
                    NSForegroundColorAttributeName,
                    NSParagraphStyleAttributeName,
                ],
                &[
                    font.into_super().into_super(),
                    color.into_super().into_super(),
                    paragraph.into_super().into_super().into_super(),
                ],
            );
            let credits = NSMutableAttributedString::initWithString_attributes(
                mtm.alloc(),
                &NSString::from_str(&copy),
                Some(&attributes),
            );
            let start = copy.find(address).expect("About address in credits");
            credits.addAttribute_value_range(
                NSLinkAttributeName,
                &NSString::from_str("https://github.com/RookieZoe/skill-man"),
                NSRange::new(
                    copy[..start].encode_utf16().count(),
                    address.encode_utf16().count(),
                ),
            );
            let values: Vec<Retained<AnyObject>> = vec![
                NSString::from_str("Skill Man").into_super().into_super(),
                NSString::from_str(&package.version.to_string())
                    .into_super()
                    .into_super(),
                NSString::from_str("").into_super().into_super(),
                credits.into_super().into_super().into_super(),
            ];
            let options = NSDictionary::from_retained_objects(
                &[
                    NSAboutPanelOptionApplicationName,
                    NSAboutPanelOptionApplicationVersion,
                    NSAboutPanelOptionVersion,
                    NSAboutPanelOptionCredits,
                ],
                &values,
            );
            let application = NSApplication::sharedApplication(mtm);
            application.orderFrontStandardAboutPanelWithOptions(&options);
            #[allow(deprecated)]
            application.activateIgnoringOtherApps(true);
        }
    });
}

#[cfg(not(target_os = "macos"))]
pub fn show(app: &tauri::AppHandle) {
    use tauri_plugin_dialog::DialogExt;
    app.dialog()
        .message(format!(
            "Skill Man {}\n{}\nhttps://github.com/RookieZoe/skill-man",
            app.package_info().version,
            app.package_info().authors
        ))
        .show(|_| {});
}
