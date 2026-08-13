//! App Update behavior through the public Core interface. External updater
//! transport and persisted cooldown state are replaced only at their seams.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use skill_man_lib::core::app_update::{
    AppUpdateCheck, AppUpdateError, AppUpdateOffer, AppUpdateService, DownloadedAppUpdate,
};
use skill_man_lib::seams::app_updater::{AppUpdater, AppUpdaterError};
use skill_man_lib::seams::clock::Clock;
use skill_man_lib::seams::preferences_store::{
    AppPreferences, PreferenceUpdates, PreferencesStore, PreferencesStoreError,
};
use skill_man_lib::tauri_adapter::app_update_api::AppUpdateApi;
use skill_man_lib::tauri_adapter::dto::{
    AppUpdateCheckDto, CancelAppUpdateRequestDto, CancelledAppUpdateDto, CheckAppUpdateRequestDto,
    DownloadAppUpdateRequestDto, DownloadedAppUpdateDto, InstallAppUpdateRequestDto,
    PublicErrorDto,
};

type UpdateFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, AppUpdaterError>> + Send + 'a>>;

struct FixedClock {
    unix_seconds: i64,
}

impl Clock for FixedClock {
    fn monotonic_millis(&self) -> u128 {
        0
    }

    fn unix_epoch_nanos(&self) -> u128 {
        u128::try_from(self.unix_seconds).unwrap_or_default() * 1_000_000_000
    }
}

struct MemoryPreferences {
    preferences: Mutex<AppPreferences>,
    last_app_update_check_at: Mutex<Option<i64>>,
}

impl MemoryPreferences {
    fn new(last_app_update_check_at: Option<i64>) -> Self {
        Self {
            preferences: Mutex::new(AppPreferences::default()),
            last_app_update_check_at: Mutex::new(last_app_update_check_at),
        }
    }
}

impl PreferencesStore for MemoryPreferences {
    fn load_preferences(&self) -> Result<AppPreferences, PreferencesStoreError> {
        Ok(self.preferences.lock().expect("Preferences lock").clone())
    }

    fn update_preferences(
        &self,
        updates: PreferenceUpdates,
    ) -> Result<AppPreferences, PreferencesStoreError> {
        let mut preferences = self.preferences.lock().expect("Preferences lock");
        if let Some(value) = updates.launch_at_login {
            preferences.launch_at_login = value;
        }
        if let Some(value) = updates.show_in_dock {
            preferences.show_in_dock = value;
        }
        if let Some(value) = updates.check_app_updates {
            preferences.check_app_updates = value;
        }
        if let Some(value) = updates.check_skill_updates {
            preferences.check_skill_updates = value;
        }
        Ok(preferences.clone())
    }

    fn last_app_update_check_at(&self) -> Result<Option<i64>, PreferencesStoreError> {
        Ok(*self
            .last_app_update_check_at
            .lock()
            .expect("last App Update check lock"))
    }

    fn record_app_update_check_at(&self, checked_at: i64) -> Result<(), PreferencesStoreError> {
        *self
            .last_app_update_check_at
            .lock()
            .expect("last App Update check lock") = Some(checked_at);
        Ok(())
    }
}

struct FakeUpdater {
    check_calls: Mutex<u32>,
    check_succeeds: bool,
    offer: Mutex<Option<AppUpdateOffer>>,
    download_succeeds: bool,
    download_calls: Mutex<u32>,
    install_calls: Mutex<u32>,
}

impl FakeUpdater {
    fn new() -> Self {
        Self {
            check_calls: Mutex::new(0),
            check_succeeds: true,
            offer: Mutex::new(None),
            download_succeeds: true,
            download_calls: Mutex::new(0),
            install_calls: Mutex::new(0),
        }
    }

    fn with_offer(offer: AppUpdateOffer) -> Self {
        Self {
            check_calls: Mutex::new(0),
            check_succeeds: true,
            offer: Mutex::new(Some(offer)),
            download_succeeds: true,
            download_calls: Mutex::new(0),
            install_calls: Mutex::new(0),
        }
    }

    fn with_download_failure(offer: AppUpdateOffer) -> Self {
        Self {
            download_succeeds: false,
            ..Self::with_offer(offer)
        }
    }

    fn with_check_failure() -> Self {
        Self {
            check_succeeds: false,
            ..Self::new()
        }
    }

    fn check_calls(&self) -> u32 {
        *self.check_calls.lock().expect("check call lock")
    }

    fn install_calls(&self) -> u32 {
        *self.install_calls.lock().expect("install call lock")
    }
}

impl AppUpdater for FakeUpdater {
    fn check(&self) -> UpdateFuture<'_, Option<AppUpdateOffer>> {
        Box::pin(async move {
            *self.check_calls.lock().expect("check call lock") += 1;
            if !self.check_succeeds {
                return Err(AppUpdaterError::SourceUnavailable(
                    "fixture is offline".into(),
                ));
            }
            Ok(self.offer.lock().expect("offer lock").clone())
        })
    }

    fn download<'a>(&'a self, _update_id: &'a str) -> UpdateFuture<'a, ()> {
        Box::pin(async move {
            *self.download_calls.lock().expect("download call lock") += 1;
            if self.download_succeeds {
                Ok(())
            } else {
                Err(AppUpdaterError::DownloadFailed(
                    "fixture signature verification failed".into(),
                ))
            }
        })
    }

    fn cancel(&self, _update_id: &str) -> Result<(), AppUpdaterError> {
        Ok(())
    }

    fn install_and_restart(&self, _update_id: &str) -> Result<(), AppUpdaterError> {
        *self.install_calls.lock().expect("install call lock") += 1;
        Ok(())
    }
}

struct BlockingCheckUpdater {
    calls: AtomicU32,
    started: Sender<()>,
    release: Mutex<Receiver<()>>,
    offer: AppUpdateOffer,
}

struct BlockingDownloadUpdater {
    offer: AppUpdateOffer,
    download_started: Sender<()>,
    cancel_download: Mutex<Option<Sender<()>>>,
    download_cancelled: Mutex<Receiver<()>>,
    cancelled: AtomicBool,
}

impl AppUpdater for BlockingDownloadUpdater {
    fn check(&self) -> UpdateFuture<'_, Option<AppUpdateOffer>> {
        Box::pin(async { Ok(Some(self.offer.clone())) })
    }

    fn download<'a>(&'a self, update_id: &'a str) -> UpdateFuture<'a, ()> {
        Box::pin(async move {
            if update_id != self.offer.update_id {
                return Err(AppUpdaterError::NoPendingUpdate);
            }
            self.download_started
                .send(())
                .expect("announce started download");
            self.download_cancelled
                .lock()
                .expect("download cancellation lock")
                .recv()
                .expect("cancel in-flight download");
            Err(AppUpdaterError::Cancelled)
        })
    }

    fn cancel(&self, update_id: &str) -> Result<(), AppUpdaterError> {
        if update_id != self.offer.update_id {
            return Err(AppUpdaterError::NoPendingUpdate);
        }
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(cancel) = self
            .cancel_download
            .lock()
            .expect("cancel download lock")
            .take()
        {
            cancel.send(()).expect("signal download cancellation");
        }
        Ok(())
    }

    fn install_and_restart(&self, _update_id: &str) -> Result<(), AppUpdaterError> {
        Ok(())
    }
}

impl AppUpdater for BlockingCheckUpdater {
    fn check(&self) -> UpdateFuture<'_, Option<AppUpdateOffer>> {
        Box::pin(async move {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call > 0 {
                return Err(AppUpdaterError::SourceUnavailable(
                    "a concurrent check crossed the Core boundary".into(),
                ));
            }
            self.started.send(()).expect("announce started check");
            self.release
                .lock()
                .expect("release lock")
                .recv()
                .expect("release first check");
            Ok(Some(self.offer.clone()))
        })
    }

    fn download<'a>(&'a self, _update_id: &'a str) -> UpdateFuture<'a, ()> {
        Box::pin(async { Ok(()) })
    }

    fn cancel(&self, _update_id: &str) -> Result<(), AppUpdaterError> {
        Ok(())
    }

    fn install_and_restart(&self, _update_id: &str) -> Result<(), AppUpdaterError> {
        Ok(())
    }
}

#[test]
fn automatic_app_update_check_skips_during_the_twenty_four_hour_cooldown() {
    let now = 2_000_000_i64;
    let preferences = Arc::new(MemoryPreferences::new(Some(now - 60 * 60)));
    let updater = Arc::new(FakeUpdater::new());
    let service = AppUpdateService::new(
        updater.clone(),
        preferences,
        Arc::new(FixedClock { unix_seconds: now }),
    );

    let result = tauri::async_runtime::block_on(service.check(false)).expect("check App Update");

    assert_eq!(result, AppUpdateCheck::SkippedCooldown);
    assert_eq!(updater.check_calls(), 0, "cooldown avoids the network seam");
}

#[test]
fn forced_app_update_check_bypasses_cooldown_and_returns_complete_offer() {
    let now = 2_000_000_i64;
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Safer updates and a clearer Library Desk.".into(),
        download_size_bytes: 8_388_608,
    };
    let preferences = Arc::new(MemoryPreferences::new(Some(now - 60 * 60)));
    let updater = Arc::new(FakeUpdater::with_offer(offer.clone()));
    let service = AppUpdateService::new(
        updater.clone(),
        preferences.clone(),
        Arc::new(FixedClock { unix_seconds: now }),
    );

    let result = tauri::async_runtime::block_on(service.check(true)).expect("force App Update");

    assert_eq!(result, AppUpdateCheck::Available(offer));
    assert_eq!(updater.check_calls(), 1);
    assert_eq!(
        preferences
            .last_app_update_check_at()
            .expect("last App Update check"),
        Some(now),
        "a successful check starts the next cooldown"
    );
}

#[test]
fn a_second_check_cannot_replace_an_offer_while_the_first_check_is_in_flight() {
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let updater = Arc::new(BlockingCheckUpdater {
        calls: AtomicU32::new(0),
        started: started_tx,
        release: Mutex::new(release_rx),
        offer: offer.clone(),
    });
    let service = Arc::new(AppUpdateService::new(
        updater.clone(),
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock {
            unix_seconds: 2_000_000,
        }),
    ));
    let first_service = service.clone();
    let first = thread::spawn(move || tauri::async_runtime::block_on(first_service.check(true)));
    started_rx.recv().expect("first check reached updater seam");

    let second = tauri::async_runtime::block_on(service.check(true));
    release_tx.send(()).expect("release first check");
    let first = first.join().expect("join first check");

    assert!(matches!(second, Err(AppUpdateError::InvalidState(_))));
    assert_eq!(
        first.expect("first check succeeds"),
        AppUpdateCheck::Available(offer)
    );
    assert_eq!(updater.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_or_unverified_download_cannot_be_installed() {
    let now = 2_000_000_i64;
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let preferences = Arc::new(MemoryPreferences::new(None));
    let updater = Arc::new(FakeUpdater::with_download_failure(offer.clone()));
    let service = AppUpdateService::new(
        updater.clone(),
        preferences,
        Arc::new(FixedClock { unix_seconds: now }),
    );
    tauri::async_runtime::block_on(service.check(true)).expect("offer App Update");

    let download_error = tauri::async_runtime::block_on(service.download(&offer.update_id))
        .expect_err("signature failure keeps the offer undownloaded");
    assert!(matches!(download_error, AppUpdateError::Updater(_)));

    let install_error = service
        .install_and_restart(&offer.update_id)
        .expect_err("an unverified package cannot be installed");
    assert!(matches!(install_error, AppUpdateError::InvalidState(_)));
    assert_eq!(updater.install_calls(), 0);
}

#[test]
fn verified_download_can_be_installed_and_restarted_exactly_once() {
    let now = 2_000_000_i64;
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let updater = Arc::new(FakeUpdater::with_offer(offer.clone()));
    let service = AppUpdateService::new(
        updater.clone(),
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock { unix_seconds: now }),
    );
    tauri::async_runtime::block_on(service.check(true)).expect("offer App Update");

    let downloaded = tauri::async_runtime::block_on(service.download(&offer.update_id))
        .expect("download and verify App Update");
    assert_eq!(
        downloaded,
        DownloadedAppUpdate {
            update_id: offer.update_id.clone(),
            version: offer.version.clone(),
        }
    );

    service
        .install_and_restart(&offer.update_id)
        .expect("install and restart");
    let second_install = service
        .install_and_restart(&offer.update_id)
        .expect_err("the retained package is consumed after installation");
    assert!(matches!(second_install, AppUpdateError::InvalidState(_)));
    assert_eq!(updater.install_calls(), 1);
}

#[test]
fn cancelling_an_in_flight_download_prevents_it_from_becoming_installable() {
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let (started_tx, started_rx) = mpsc::channel();
    let (cancel_tx, cancel_rx) = mpsc::channel();
    let updater = Arc::new(BlockingDownloadUpdater {
        offer: offer.clone(),
        download_started: started_tx,
        cancel_download: Mutex::new(Some(cancel_tx)),
        download_cancelled: Mutex::new(cancel_rx),
        cancelled: AtomicBool::new(false),
    });
    let service = Arc::new(AppUpdateService::new(
        updater.clone(),
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock {
            unix_seconds: 2_000_000,
        }),
    ));
    tauri::async_runtime::block_on(service.check(true)).expect("offer App Update");
    let download_service = service.clone();
    let update_id = offer.update_id.clone();
    let download = thread::spawn(move || {
        tauri::async_runtime::block_on(download_service.download(&update_id))
    });
    started_rx
        .recv()
        .expect("download reached the updater seam");

    let cancelled = service
        .cancel(&offer.update_id)
        .expect("cancel App Update download");
    let download = download.join().expect("join cancelled download");

    assert_eq!(cancelled.update_id, offer.update_id);
    assert!(matches!(
        download,
        Err(AppUpdateError::Updater(AppUpdaterError::Cancelled))
    ));
    assert!(updater.cancelled.load(Ordering::SeqCst));
    assert!(matches!(
        service.install_and_restart(&offer.update_id),
        Err(AppUpdateError::InvalidState(_))
    ));
}

#[test]
fn cancelling_a_downloaded_update_discards_it_and_allows_a_fresh_check() {
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let service = AppUpdateService::new(
        Arc::new(FakeUpdater::with_offer(offer.clone())),
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock {
            unix_seconds: 2_000_000,
        }),
    );
    tauri::async_runtime::block_on(service.check(true)).expect("offer App Update");
    tauri::async_runtime::block_on(service.download(&offer.update_id))
        .expect("download App Update");

    let cancelled = service
        .cancel(&offer.update_id)
        .expect("discard downloaded App Update");

    assert_eq!(cancelled.update_id, offer.update_id);
    assert_eq!(
        tauri::async_runtime::block_on(service.check(true)).expect("fresh App Update check"),
        AppUpdateCheck::Available(offer)
    );
}

#[test]
fn automatic_and_manual_check_failures_keep_the_same_typed_command_error() {
    let updater = Arc::new(FakeUpdater::with_check_failure());
    let api = AppUpdateApi::new(AppUpdateService::new(
        updater,
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock {
            unix_seconds: 2_000_000,
        }),
    ));

    let automatic = tauri::async_runtime::block_on(
        api.check_app_update(CheckAppUpdateRequestDto { force: false }),
    )
    .expect_err("React decides whether an automatic command error is silent");
    assert!(matches!(automatic.error, PublicErrorDto::SourceUnavailable));

    let manual = tauri::async_runtime::block_on(
        api.check_app_update(CheckAppUpdateRequestDto { force: true }),
    )
    .expect_err("manual check failure is shown to the user");
    assert!(matches!(manual.error, PublicErrorDto::SourceUnavailable));
}

#[test]
fn typed_app_update_check_dto_is_flat_and_uses_the_confirmed_statuses() {
    let available = AppUpdateCheckDto::from(AppUpdateCheck::Available(AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    }));

    assert_eq!(
        serde_json::to_value(available).expect("serialize available App Update"),
        serde_json::json!({
            "status": "available",
            "updateId": "offer-0.2.0",
            "currentVersion": "0.1.0",
            "version": "0.2.0",
            "releaseNotes": "Release notes",
            "downloadSizeBytes": 8_388_608,
        })
    );
    assert_eq!(
        serde_json::to_value(AppUpdateCheckDto::from(AppUpdateCheck::SkippedCooldown))
            .expect("serialize skipped App Update check"),
        serde_json::json!({ "status": "skipped" })
    );
    assert_eq!(
        serde_json::to_value(AppUpdateCheckDto::from(AppUpdateCheck::UpToDate))
            .expect("serialize up-to-date App Update check"),
        serde_json::json!({ "status": "up_to_date" })
    );
}

#[test]
fn typed_app_update_api_keeps_download_and_install_as_separate_confirmed_phases() {
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let api = AppUpdateApi::new(AppUpdateService::new(
        Arc::new(FakeUpdater::with_offer(offer.clone())),
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock {
            unix_seconds: 2_000_000,
        }),
    ));
    tauri::async_runtime::block_on(api.check_app_update(CheckAppUpdateRequestDto { force: true }))
        .expect("offer App Update");

    let downloaded =
        tauri::async_runtime::block_on(api.download_app_update(DownloadAppUpdateRequestDto {
            update_id: offer.update_id.clone(),
        }))
        .expect("download App Update");
    assert_eq!(
        downloaded,
        DownloadedAppUpdateDto {
            update_id: offer.update_id.clone(),
            version: offer.version,
        }
    );

    api.install_app_update(InstallAppUpdateRequestDto {
        update_id: offer.update_id,
    })
    .expect("install and restart App Update");
}

#[test]
fn typed_app_update_api_returns_the_cancelled_update_identity() {
    let offer = AppUpdateOffer {
        update_id: "offer-0.2.0".into(),
        current_version: "0.1.0".into(),
        version: "0.2.0".into(),
        release_notes: "Release notes".into(),
        download_size_bytes: 8_388_608,
    };
    let api = AppUpdateApi::new(AppUpdateService::new(
        Arc::new(FakeUpdater::with_offer(offer.clone())),
        Arc::new(MemoryPreferences::new(None)),
        Arc::new(FixedClock {
            unix_seconds: 2_000_000,
        }),
    ));
    tauri::async_runtime::block_on(api.check_app_update(CheckAppUpdateRequestDto { force: true }))
        .expect("offer App Update");

    assert_eq!(
        api.cancel_app_update(CancelAppUpdateRequestDto {
            update_id: offer.update_id.clone(),
        })
        .expect("cancel App Update"),
        CancelledAppUpdateDto {
            update_id: offer.update_id,
        }
    );
}
