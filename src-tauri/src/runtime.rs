use crate::core::{
    digest::DigestState,
    history::HistoryStore,
    model::Account,
    notifications::AlertMemory,
    poller::Schedule,
    providers,
    settings::Settings,
    store::{self, StoredState},
    switch, tray,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, Notify};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppState {
    pub revision: u64,
    pub accounts: Vec<Account>,
    pub settings: Settings,
    pub refreshing: bool,
    pub notice: Option<String>,
    pub bridge_notice: Option<String>,
    pub recovery: bool,
    pub recovered: Vec<String>,
    pub ignored_sources: Vec<String>,
    pub switching: SwitchView,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchView {
    pub active_account_ids: Vec<String>,
    pub restore: Option<switch::RestoreOffer>,
}

impl AppState {
    pub fn changed(&mut self) {
        self.revision = self.revision.saturating_add(1);
    }
}

pub struct RuntimeState {
    pub view: AppState,
    pub alerts: AlertMemory,
    pub schedules: HashMap<String, Schedule>,
    pub history: HistoryStore,
}

pub struct Runtime {
    pub inner: Mutex<RuntimeState>,
    pub client: reqwest::Client,
    pub profiles: providers::claude::ProfileCache,
    pub resets: providers::codex::ResetCreditCache,
    pub wake: Notify,
    pub update_wake: Notify,
    pub first_run: bool,
    store_path: PathBuf,
    recovery_error: std::sync::Mutex<Option<String>>,
    backup_pending: AtomicBool,
    last_update_check_at: std::sync::Mutex<Option<String>>,
    digest: std::sync::Mutex<Option<DigestState>>,
    pub switching: AtomicBool,
    save_pending: AtomicBool,
    last_saved: std::sync::Mutex<Option<Instant>>,
}

/// New readings reach the settings file at most this often; alerts and user
/// changes are saved at once.
pub const READING_SAVE_INTERVAL: Duration = Duration::from_secs(30);

pub fn reading_save_due(last_saved: Option<Instant>, now: Instant) -> bool {
    last_saved.is_none_or(|last| now.saturating_duration_since(last) >= READING_SAVE_INTERVAL)
}

impl Runtime {
    pub fn new(store_path: PathBuf) -> Result<Self, String> {
        let first_run = !store_path.exists();
        let (saved, recovery_error) = match store::load(&store_path) {
            Ok(saved) => (saved, None),
            Err(error) => (
                StoredState {
                    version: 1,
                    ..StoredState::default()
                },
                Some(format!("{error} Settings file: {}.", store_path.display())),
            ),
        };
        let mut state = RuntimeState {
            view: AppState {
                revision: 0,
                accounts: saved.accounts,
                settings: saved.settings,
                refreshing: false,
                notice: recovery_error.clone(),
                bridge_notice: None,
                recovery: recovery_error.is_some(),
                recovered: saved.recovered.clone(),
                ignored_sources: saved.ignored_sources,
                switching: SwitchView::default(),
            },
            alerts: saved.alerts,
            schedules: HashMap::new(),
            history: HistoryStore::new(store_path.with_file_name("history")),
        };
        if recovery_error.is_none() {
            crate::make_active::recover(&mut state, &mut journal(&store_path));
        }
        state.view.switching.active_account_ids = switch::active_ids(&state.view.accounts);
        Ok(Self {
            inner: Mutex::new(state),
            client: providers::http_client().map_err(|error| error.message)?,
            profiles: Default::default(),
            resets: Default::default(),
            wake: Notify::new(),
            update_wake: Notify::new(),
            first_run: first_run && recovery_error.is_none(),
            store_path,
            recovery_error: std::sync::Mutex::new(recovery_error),
            backup_pending: AtomicBool::new(!saved.recovered.is_empty()),
            last_update_check_at: std::sync::Mutex::new(saved.last_update_check_at),
            digest: std::sync::Mutex::new(saved.digest),
            switching: AtomicBool::new(false),
            save_pending: AtomicBool::new(false),
            last_saved: std::sync::Mutex::new(None),
        })
    }

    pub fn journal(&self) -> switch::DiskJournal {
        journal(&self.store_path)
    }

    pub fn ensure_store_available(&self) -> Result<(), String> {
        match &*self
            .recovery_error
            .lock()
            .map_err(|_| "Settings are unavailable.")?
        {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }

    pub async fn reset_store(&self) -> Result<AppState, String> {
        let mut inner = self.inner.lock().await;
        let mut next = AppState {
            revision: inner.view.revision,
            accounts: Vec::new(),
            settings: Settings {
                autostart: inner.view.settings.autostart,
                ..Settings::default()
            },
            refreshing: false,
            notice: None,
            bridge_notice: inner.view.bridge_notice.clone(),
            recovery: false,
            recovered: Vec::new(),
            ignored_sources: Vec::new(),
            switching: SwitchView::default(),
        };
        next.changed();
        store::reset(
            &self.store_path,
            &StoredState {
                version: 1,
                settings: next.settings.clone(),
                ..StoredState::default()
            },
        )?;
        *self
            .recovery_error
            .lock()
            .map_err(|_| "Settings are unavailable.")? = None;
        self.backup_pending.store(false, Ordering::SeqCst);
        inner.view = next.clone();
        inner.alerts.clear();
        inner.schedules.clear();
        self.set_digest_state(None);
        Ok(next)
    }

    pub fn persist(&self, view: &AppState, alerts: &AlertMemory) -> Result<(), String> {
        self.ensure_store_available()?;
        if self.backup_pending.swap(false, Ordering::SeqCst) {
            if let Err(error) = store::backup(&self.store_path) {
                self.backup_pending.store(true, Ordering::SeqCst);
                return Err(error);
            }
        }
        store::save(
            &self.store_path,
            &StoredState {
                version: 1,
                accounts: view.accounts.clone(),
                settings: view.settings.clone(),
                alerts: alerts.clone(),
                ignored_sources: view.ignored_sources.clone(),
                last_update_check_at: self.last_update_check_at(),
                digest: self.digest_state(),
                ..StoredState::default()
            },
        )?;
        self.save_pending.store(false, Ordering::SeqCst);
        if let Ok(mut last) = self.last_saved.lock() {
            *last = Some(Instant::now());
        }
        Ok(())
    }

    /// Saves a new reading now when `urgent` or when the last save is old
    /// enough; otherwise leaves it for the poller to flush. `None` means deferred.
    pub fn persist_reading(
        &self,
        view: &AppState,
        alerts: &AlertMemory,
        urgent: bool,
    ) -> Option<Result<(), String>> {
        if urgent || self.reading_save_due() {
            return Some(self.persist(view, alerts));
        }
        self.save_pending.store(true, Ordering::SeqCst);
        None
    }

    /// True when a deferred reading is waiting and the interval has passed.
    pub fn flush_due(&self) -> bool {
        self.save_pending.load(Ordering::SeqCst) && self.reading_save_due()
    }

    pub fn has_pending_save(&self) -> bool {
        self.save_pending.load(Ordering::SeqCst)
    }

    fn reading_save_due(&self) -> bool {
        let last = self.last_saved.lock().ok().and_then(|last| *last);
        reading_save_due(last, Instant::now())
    }

    pub fn digest_state(&self) -> Option<DigestState> {
        self.digest.lock().ok().and_then(|value| value.clone())
    }

    pub fn set_digest_state(&self, state: Option<DigestState>) {
        if let Ok(mut digest) = self.digest.lock() {
            *digest = state;
        }
    }

    pub fn last_update_check_at(&self) -> Option<String> {
        self.last_update_check_at
            .lock()
            .ok()
            .and_then(|value| value.clone())
    }

    pub async fn record_update_check(&self, at: String) -> Result<(), String> {
        *self
            .last_update_check_at
            .lock()
            .map_err(|_| "Update check time unavailable.")? = Some(at);
        let inner = self.inner.lock().await;
        self.persist(&inner.view, &inner.alerts)
    }

    pub async fn change<F>(&self, app: &AppHandle, change: F) -> Result<AppState, String>
    where
        F: FnOnce(&mut AppState) -> Result<(), String>,
    {
        let mut inner = self.inner.lock().await;
        let mut next = inner.view.clone();
        change(&mut next)?;
        next.switching.active_account_ids = switch::active_ids(&next.accounts);
        next.refreshing = next.accounts.iter().any(|account| account.refreshing);
        next.notice = None;
        next.changed();
        let mut alerts = inner.alerts.clone();
        alerts.retain(|key, _| {
            next.accounts
                .iter()
                .any(|account| key.starts_with(&format!("{}/", account.id)))
        });
        self.ensure_store_available()?;
        let changed_ids: Vec<String> = inner
            .view
            .accounts
            .iter()
            .filter(|account| !next.accounts.iter().any(|next| next.id == account.id))
            .chain(
                next.accounts
                    .iter()
                    .filter(|account| !inner.view.accounts.iter().any(|old| old.id == account.id)),
            )
            .map(|account| account.id.clone())
            .collect();
        // Save first: if that fails, the account and its history both stay.
        self.persist(&next, &alerts)?;
        for id in changed_ids {
            if let Err(error) = inner.history.remove(&id) {
                next.notice = Some(error);
            }
        }
        inner.view = next.clone();
        inner.alerts = alerts;
        inner
            .schedules
            .retain(|id, _| next.accounts.iter().any(|account| &account.id == id));
        drop(inner);
        publish(app, &next);
        self.wake.notify_one();
        Ok(next)
    }
}

fn journal(store_path: &std::path::Path) -> switch::DiskJournal {
    switch::DiskJournal {
        dir: store_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_default(),
        keychain: cfg!(target_os = "macos"),
    }
}

pub fn publish(app: &AppHandle, state: &AppState) {
    tray::update(app, state);
    let _ = app.emit("state_changed", state);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_are_saved_at_most_every_interval() {
        let now = Instant::now();
        assert!(reading_save_due(None, now));
        assert!(!reading_save_due(Some(now), now));
        assert!(!reading_save_due(
            Some(now),
            now + READING_SAVE_INTERVAL - Duration::from_millis(1)
        ));
        assert!(reading_save_due(Some(now), now + READING_SAVE_INTERVAL));
    }

    #[tokio::test]
    async fn existing_settings_remain_usable_when_separate_history_is_corrupt() {
        use crate::core::model::{AccountSource, Identity, Provider};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let account = Account {
            id: "codex-test".into(),
            provider: Provider::Codex,
            label: "Work".into(),
            color: "#ffffff".into(),
            source: AccountSource::ConfigDir {
                path: directory.path().to_string_lossy().into_owned(),
                keychain_suffix: None,
            },
            identity: Identity::default(),
            snapshot: None,
            error: None,
            refreshing: false,
        };
        let saved = StoredState {
            version: 1,
            accounts: vec![account.clone()],
            ..StoredState::default()
        };
        store::save(&path, &saved).unwrap();
        std::fs::create_dir(directory.path().join("history")).unwrap();
        let history_path = directory.path().join("history/codex-test.json");
        std::fs::write(&history_path, "{damaged").unwrap();
        let runtime = Runtime::new(path).unwrap();
        let mut inner = runtime.inner.lock().await;
        assert_eq!(inner.view.accounts.len(), 1);
        assert!(inner.view.notice.is_none());
        assert!(inner
            .history
            .get(&account, chrono::Utc::now())
            .error
            .is_some());
        assert!(runtime.persist(&inner.view, &inner.alerts).is_ok());
        assert_eq!(std::fs::read_to_string(history_path).unwrap(), "{damaged");
    }

    #[tokio::test]
    async fn reset_keeps_panel_recovery_notice() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = Runtime::new(directory.path().join("state.json")).unwrap();
        runtime.inner.lock().await.view.bridge_notice = Some("Restart Margin".into());
        let view = runtime.reset_store().await.unwrap();
        assert_eq!(view.bridge_notice.as_deref(), Some("Restart Margin"));
    }

    #[tokio::test]
    async fn damaged_settings_remain_untouched_and_visible_for_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        std::fs::write(&path, "{damaged").unwrap();
        let runtime = Runtime::new(path.clone()).unwrap();
        let inner = runtime.inner.lock().await;
        assert!(!runtime.first_run);
        assert!(inner.view.accounts.is_empty());
        assert!(inner.view.notice.as_ref().unwrap().contains("state.json"));
        assert!(inner.view.recovery);
        assert!(runtime.persist(&inner.view, &inner.alerts).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{damaged");
        drop(inner);
        let view = runtime.reset_store().await.unwrap();
        assert!(!view.recovery && view.notice.is_none());
        assert!(runtime.ensure_store_available().is_ok());
        assert!(store::load(&path).unwrap().accounts.is_empty());
        assert_eq!(backups(directory.path()), vec!["{damaged".to_string()]);
    }

    #[tokio::test]
    async fn dropping_a_corrupt_account_backs_up_once_before_writing() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        let original = serde_json::json!({
            "version": 1,
            "accounts": [{"id": "broken"}],
            "settings": {}
        })
        .to_string();
        std::fs::write(&path, &original).unwrap();
        let runtime = Runtime::new(path.clone()).unwrap();
        let inner = runtime.inner.lock().await;
        assert!(!inner.view.recovery);
        assert_eq!(inner.view.recovered.len(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        runtime.persist(&inner.view, &inner.alerts).unwrap();
        runtime.persist(&inner.view, &inner.alerts).unwrap();
        assert_eq!(backups(directory.path()), vec![original]);
        assert!(store::load(&path).unwrap().recovered.is_empty());
    }

    fn backups(directory: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.to_string_lossy().contains("state.json.bak-"))
            .map(|path| std::fs::read_to_string(path).unwrap())
            .collect()
    }
}
