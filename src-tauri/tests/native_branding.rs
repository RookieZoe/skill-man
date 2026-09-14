//! Runs on the process main thread so Cocoa's actual presentation objects can be inspected.
#[cfg(target_os = "macos")]
fn main() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSAboutPanelOptionApplicationIcon, NSApplication, NSImage};
    use skill_man_lib::tauri_adapter::native_branding::{about_options, app_icon, message_alert};
    let mtm = MainThreadMarker::new().expect("native test main thread");
    let _app = NSApplication::sharedApplication(mtm);
    let expected = app_icon(mtm).TIFFRepresentation().expect("brand pixels");
    let options = about_options(mtm, &[], &[]);
    let about_matches = options
        .objectForKey(unsafe { NSAboutPanelOptionApplicationIcon })
        .and_then(|icon| icon.downcast::<NSImage>().ok())
        .and_then(|icon| icon.TIFFRepresentation())
        .is_some_and(|pixels| pixels.to_vec() == expected.to_vec());
    let alert_matches = message_alert(mtm)
        .icon()
        .and_then(|icon| icon.TIFFRepresentation())
        .is_some_and(|pixels| pixels.to_vec() == expected.to_vec());
    assert!(
        about_matches && alert_matches,
        "unbundled native surfaces must use embedded brand pixels: About={about_matches}, alert={alert_matches}"
    );
    println!("About and native alert use the embedded Skill Man icon without an App Bundle");
}
#[cfg(not(target_os = "macos"))]
fn main() {}
