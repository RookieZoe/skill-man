//! Native presentation factories with an explicit, bundle-independent brand icon.
use objc2::{MainThreadMarker, rc::Retained, runtime::AnyObject};
use objc2_app_kit::{NSAlert, NSImage};
use objc2_foundation::{NSData, NSDictionary, NSString};

pub fn app_icon(mtm: MainThreadMarker) -> Retained<NSImage> {
    NSImage::initWithData(
        mtm.alloc(),
        &NSData::with_bytes(include_bytes!("../../icons/icon.icns")),
    )
    .expect("embedded Skill Man icon must decode")
}

pub fn about_options(
    mtm: MainThreadMarker,
    keys: &[&NSString],
    values: &[Retained<AnyObject>],
) -> Retained<NSDictionary<NSString, AnyObject>> {
    let mut keys = keys.to_vec();
    let mut values = values.to_vec();
    // SAFETY: Cocoa defines this key for the About panel's NSImage value.
    keys.push(unsafe { objc2_app_kit::NSAboutPanelOptionApplicationIcon });
    values.push(app_icon(mtm).into_super().into_super());
    NSDictionary::from_retained_objects(&keys, &values)
}

pub fn message_alert(mtm: MainThreadMarker) -> Retained<NSAlert> {
    let alert = NSAlert::new(mtm);
    // SAFETY: Always supply a decoded, non-nil NSImage.
    unsafe { alert.setIcon(Some(&app_icon(mtm))) };
    alert
}
