//! AppKit progress window. All retained Objective-C objects stay on the main thread.
use super::{
    app_update_api::AppUpdateApi,
    dto::CancelAppUpdateRequestDto,
    native_app_update::{NativeAppUpdate, text},
    native_message::NativeMessageKey as Key,
};
use objc2::{
    DefinedClass, MainThreadOnly, define_class, msg_send, rc::Retained, runtime::ProtocolObject,
};
use objc2_app_kit::{
    NSAlert, NSAlertFirstButtonReturn, NSApplication, NSBackingStoreType, NSProgressIndicator,
    NSTextField, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};
use std::{
    cell::RefCell,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Manager};

struct CloseState {
    app: AppHandle,
    cancelled: Arc<AtomicBool>,
    update_id: Option<String>,
}
define_class!(
    // SAFETY: NSObject has no subclassing requirements; this class is main-thread-only.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = CloseState]
    struct ProgressDelegate;
    unsafe impl NSObjectProtocol for ProgressDelegate {}
    unsafe impl NSWindowDelegate for ProgressDelegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            self.ivars()
                .app
                .state::<NativeAppUpdate>()
                .cancel_progress(&self.ivars().cancelled);
            if let Some(update_id) = &self.ivars().update_id {
                let _ = self.ivars().app.state::<AppUpdateApi>().cancel_app_update(
                    CancelAppUpdateRequestDto {
                        update_id: update_id.clone(),
                    },
                );
            }
        }
    }
);
impl ProgressDelegate {
    fn new(mtm: MainThreadMarker, state: CloseState) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(state);
        // SAFETY: NSObject init has this signature.
        unsafe { msg_send![super(this), init] }
    }
}
struct ProgressWindow {
    window: Retained<NSWindow>,
    delegate: Retained<ProgressDelegate>,
    label: Retained<NSTextField>,
    message: Key,
}
thread_local! {
    static WINDOW: RefCell<Option<ProgressWindow>> = const { RefCell::new(None) };
}

pub async fn show(
    app: &AppHandle,
    message: Key,
    cancelled: Arc<AtomicBool>,
    update_id: Option<String>,
) -> Result<(), String> {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    let owner = app.clone();
    app.run_on_main_thread(move || {
        let mtm = MainThreadMarker::new().expect("Tauri main thread");
        // A promoted automatic check may finish before this main-thread callback runs.
        if cancelled.load(Ordering::SeqCst)
            || (update_id.is_none() && !owner.state::<NativeAppUpdate>().is_checking())
        {
            let _ = tx.try_send(());
            return;
        }
        hide_on_main();
        // SAFETY: AppKit is called on the main thread and automatic release on close is disabled.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(380., 112.)),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(&text(&owner, Key::CheckUpdate, &[])));
        let view = window.contentView().expect("progress window content view");
        let label = NSTextField::wrappingLabelWithString(
            &NSString::from_str(&text(&owner, message, &[])),
            mtm,
        );
        // SAFETY: Views are retained by the window and are only accessed on the main thread.
        unsafe {
            label.setFrame(NSRect::new(NSPoint::new(24., 55.), NSSize::new(332., 36.)));
            view.addSubview(&label);
            let progress = NSProgressIndicator::initWithFrame(
                NSProgressIndicator::alloc(mtm),
                NSRect::new(NSPoint::new(24., 28.), NSSize::new(332., 14.)),
            );
            progress.setIndeterminate(true);
            progress.startAnimation(None);
            view.addSubview(&progress);
        }
        let delegate = ProgressDelegate::new(
            mtm,
            CloseState {
                app: owner,
                cancelled,
                update_id,
            },
        );
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        window.center();
        window.makeKeyAndOrderFront(None);
        #[allow(deprecated)]
        NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
        WINDOW.with(|slot| {
            *slot.borrow_mut() = Some(ProgressWindow {
                window,
                delegate,
                label,
                message,
            })
        });
        let _ = tx.try_send(());
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .await
        .ok_or_else(|| "native_update_window_unavailable".into())
}
fn hide_on_main() {
    WINDOW.with(|slot| {
        if let Some(progress) = slot.borrow_mut().take() {
            progress.window.setDelegate(None);
            progress.window.orderOut(None);
        }
    });
}
pub async fn hide(app: &AppHandle) {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    if app
        .run_on_main_thread(move || {
            hide_on_main();
            let _ = tx.try_send(());
        })
        .is_ok()
    {
        let _ = rx.recv().await;
    }
}
pub fn focus(app: &AppHandle) {
    let _ = app.run_on_main_thread(|| {
        WINDOW.with(|slot| {
            if let Some(progress) = slot.borrow().as_ref() {
                if !progress.delegate.ivars().cancelled.load(Ordering::SeqCst) {
                    progress.window.makeKeyAndOrderFront(None);
                }
            }
        });
        #[allow(deprecated)]
        NSApplication::sharedApplication(MainThreadMarker::new().expect("Tauri main thread"))
            .activateIgnoringOtherApps(true);
    });
}

pub fn refresh_locale(app: &AppHandle) {
    let owner = app.clone();
    let _ = app.run_on_main_thread(move || {
        WINDOW.with(|slot| {
            if let Some(progress) = slot.borrow().as_ref() {
                progress
                    .window
                    .setTitle(&NSString::from_str(&text(&owner, Key::CheckUpdate, &[])));
                progress.label.setStringValue(&NSString::from_str(&text(
                    &owner,
                    progress.message,
                    &[],
                )));
            }
        });
    });
}

/// An app-modal alert must not attach to a hidden main/progress window.
/// Tauri's asynchronous dialog plugin implicitly chooses a parent on macOS.
pub async fn prompt(
    app: &AppHandle,
    title: String,
    message: String,
    accept: String,
    cancel: Option<String>,
) -> bool {
    let (tx, mut rx) = tauri::async_runtime::channel(1);
    if app
        .run_on_main_thread(move || {
            let mtm = MainThreadMarker::new().expect("Tauri main thread");
            // The alert is created, run and released on the main thread.
            // runModal owns the nested AppKit event loop until the user responds.
            let result = {
                let alert = NSAlert::new(mtm);
                alert.setMessageText(&NSString::from_str(&title));
                alert.setInformativeText(&NSString::from_str(&message));
                alert.addButtonWithTitle(&NSString::from_str(&accept));
                if let Some(cancel) = cancel {
                    alert.addButtonWithTitle(&NSString::from_str(&cancel));
                }
                #[allow(deprecated)]
                NSApplication::sharedApplication(mtm).activateIgnoringOtherApps(true);
                alert.runModal() == NSAlertFirstButtonReturn
            };
            let _ = tx.try_send(result);
        })
        .is_err()
    {
        return false;
    }
    rx.recv().await.unwrap_or(false)
}
