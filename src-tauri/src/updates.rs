use crate::runtime::Runtime;
use chrono::{DateTime, TimeDelta, Utc};
use serde::Serialize;
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};

#[derive(Clone, Debug, Default, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum UpdatePhase {
    #[default]
    Idle,
    Checking,
    Current,
    Available {
        version: String,
    },
    Downloading {
        version: String,
        downloaded: u64,
        total: Option<u64>,
    },
    Installing {
        version: String,
    },
    Error {
        message: String,
    },
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct UpdateStatus {
    revision: u64,
    state: UpdatePhase,
}

#[derive(Default)]
pub struct Updates {
    status: Mutex<UpdateStatus>,
    pending: tokio::sync::Mutex<Option<Update>>,
}

impl Updates {
    fn status(&self) -> Result<UpdateStatus, String> {
        self.status
            .lock()
            .map(|status| status.clone())
            .map_err(|_| "Update status unavailable. Reopen Margin and try again.".into())
    }

    fn publish(&self, app: &AppHandle, state: UpdatePhase) -> Result<UpdateStatus, String> {
        let mut status = self
            .status
            .lock()
            .map_err(|_| "Update status unavailable. Reopen Margin and try again.")?;
        status.revision = status.revision.saturating_add(1);
        status.state = state;
        let _ = app.emit("update_status_changed", status.clone());
        Ok(status.clone())
    }

    fn fail(&self, app: &AppHandle, message: &str) -> Result<UpdateStatus, String> {
        self.publish(
            app,
            UpdatePhase::Error {
                message: message.into(),
            },
        )
    }
}

fn trusted_download(url: &reqwest::Url, version: &str) -> bool {
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let path = url.path();
    let Some(file) = ["/Abulkosim/margin/", "/Abulkosim/usagebar/"]
        .iter()
        .find_map(|repo| path.strip_prefix(&format!("{repo}releases/download/v{version}/")))
    else {
        return false;
    };
    file == "Margin.app.tar.gz"
        || file == format!("Margin_{version}_x64-setup.exe")
        || file == format!("Margin_{version}_x64_en-US.msi")
}

#[tauri::command]
pub fn get_update_status(updates: State<'_, Updates>) -> Result<UpdateStatus, String> {
    updates.status()
}

#[tauri::command]
pub async fn check_for_update(
    app: AppHandle,
    updates: State<'_, Updates>,
) -> Result<UpdateStatus, String> {
    check(&app, &updates, false).await
}

async fn check(app: &AppHandle, updates: &Updates, quiet: bool) -> Result<UpdateStatus, String> {
    if cfg!(target_os = "linux") {
        return updates.fail(app, "Updates are not available for this Linux build. Update it through your package source.");
    }
    let Ok(mut pending) = updates.pending.try_lock() else {
        return updates.status();
    };
    let previous = updates.status()?.state;
    *pending = None;
    updates.publish(app, UpdatePhase::Checking)?;
    let result = match app
        .updater_builder()
        .timeout(Duration::from_secs(20))
        .build()
    {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    let message = match result {
        Ok(Some(mut update)) if trusted_download(&update.download_url, &update.version) => {
            update.timeout = Some(Duration::from_secs(300));
            let version = update.version.clone();
            *pending = Some(update);
            return updates.publish(app, UpdatePhase::Available { version });
        }
        Ok(None) => return updates.publish(app, UpdatePhase::Current),
        Ok(Some(_)) => "This update has an unexpected download address. Try again after the release is corrected.",
        Err(_) => "Couldn’t check for updates. Check your connection and try again.",
    };
    if quiet {
        #[cfg(debug_assertions)]
        eprintln!("Background update check failed: {message}");
        return updates.publish(app, previous);
    }
    updates.fail(app, message)
}

const FIRST_CHECK_DELAY: Duration = Duration::from_secs(120);
const DUE_CHECK_INTERVAL: Duration = Duration::from_secs(15 * 60);

pub fn due(last: Option<&str>, now: DateTime<Utc>) -> bool {
    let Some(last) = last.and_then(|value| DateTime::parse_from_rfc3339(value).ok()) else {
        return true;
    };
    let elapsed = now.signed_duration_since(last);
    elapsed < TimeDelta::zero() || elapsed >= TimeDelta::hours(24)
}

pub fn start_background_checks(app: AppHandle, runtime: Arc<Runtime>) {
    if cfg!(target_os = "linux") {
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK_DELAY).await;
        loop {
            background_check(&app, &runtime).await;
            tokio::select! {
                _ = tokio::time::sleep(DUE_CHECK_INTERVAL) => {}
                _ = runtime.update_wake.notified() => {}
            }
        }
    });
}

async fn background_check(app: &AppHandle, runtime: &Runtime) {
    let enabled = runtime.inner.lock().await.view.settings.auto_check_updates;
    let now = Utc::now();
    if !enabled || !due(runtime.last_update_check_at().as_deref(), now) {
        return;
    }
    let updates = app.state::<Updates>();
    if updates.status().is_ok_and(|status| {
        matches!(
            status.state,
            UpdatePhase::Checking
                | UpdatePhase::Available { .. }
                | UpdatePhase::Downloading { .. }
                | UpdatePhase::Installing { .. }
        )
    }) {
        return;
    }
    let _ = runtime.record_update_check(now.to_rfc3339()).await;
    let _ = check(app, &updates, true).await;
}

#[tauri::command]
pub async fn install_update(
    app: AppHandle,
    updates: State<'_, Updates>,
    version: String,
) -> Result<UpdateStatus, String> {
    if cfg!(target_os = "linux") {
        return updates.fail(&app, "Updates are not available for this Linux build. Update it through your package source.");
    }
    let Ok(mut pending) = updates.pending.try_lock() else {
        return updates.status();
    };
    let Some(update) = pending.take().filter(|update| update.version == version) else {
        return updates.fail(&app, "Check for updates again before installing.");
    };
    updates.publish(
        &app,
        UpdatePhase::Downloading {
            version: version.clone(),
            downloaded: 0,
            total: None,
        },
    )?;
    let mut downloaded = 0_u64;
    let mut last_event = Instant::now();
    let bytes = update
        .download(
            |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                if last_event.elapsed() >= Duration::from_millis(100) || total == Some(downloaded) {
                    let _ = updates.publish(
                        &app,
                        UpdatePhase::Downloading {
                            version: version.clone(),
                            downloaded,
                            total,
                        },
                    );
                    last_event = Instant::now();
                }
            },
            || {},
        )
        .await;
    let bytes = match bytes {
        Ok(bytes) => bytes,
        Err(_) => return updates.fail(&app, "Couldn’t download and verify this update. Nothing was installed. Check for updates to try again."),
    };
    updates.publish(&app, UpdatePhase::Installing { version })?;
    match tauri::async_runtime::spawn_blocking(move || update.install(bytes)).await {
        Ok(Ok(())) => app.restart(),
        _ => updates.fail(
            &app,
            "Couldn’t finish installing. Reopen Margin, or install the latest release from GitHub.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_are_limited_to_versioned_margin_release_assets() {
        let base = "https://github.com/Abulkosim/usagebar/releases/download/v0.2.0/";
        for file in [
            "Margin.app.tar.gz",
            "Margin_0.2.0_x64-setup.exe",
            "Margin_0.2.0_x64_en-US.msi",
        ] {
            assert!(trusted_download(
                &format!("{base}{file}").parse().unwrap(),
                "0.2.0"
            ));
            assert!(trusted_download(
                &format!("{}{file}", base.replace("usagebar", "margin"))
                    .parse()
                    .unwrap(),
                "0.2.0"
            ));
        }
        for url in [
            format!("{base}Margin_0.1.1_x64-setup.exe"),
            format!("{base}Margin.app.tar.gz?token=anything"),
            format!("{base}Margin.app.tar.gz#fragment"),
            format!("{base}../Margin.app.tar.gz"),
            format!("{base}other.exe"),
            base.replace("https:", "http:") + "Margin.app.tar.gz",
            base.replace("github.com", "github.com.evil.example") + "Margin.app.tar.gz",
            base.replace("github.com", "user:pass@github.com") + "Margin.app.tar.gz",
            base.replace("usagebar", "another-repo") + "Margin.app.tar.gz",
            base.replace("v0.2.0", "v0.1.1") + "Margin.app.tar.gz",
        ] {
            assert!(!trusted_download(&url.parse().unwrap(), "0.2.0"), "{url}");
        }
    }

    #[test]
    fn download_progress_uses_the_frontend_contract() {
        let status = UpdateStatus {
            revision: 3,
            state: UpdatePhase::Downloading {
                version: "0.2.0".into(),
                downloaded: 10,
                total: None,
            },
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::json!({"revision": 3, "state": {"kind": "downloading", "version": "0.2.0", "downloaded": 10, "total": null}})
        );
    }

    #[test]
    fn background_checks_are_due_at_most_once_a_day() {
        let now: DateTime<Utc> = "2026-10-22T12:00:00Z".parse().unwrap();
        assert!(due(None, now));
        assert!(due(Some("not a time"), now));
        assert!(due(Some(""), now));
        assert!(due(Some("2026-10-21T12:00:00Z"), now));
        assert!(due(Some("2026-10-01T00:00:00+05:00"), now));
        assert!(!due(Some("2026-10-21T12:00:01Z"), now));
        assert!(!due(Some("2026-10-22T11:59:00Z"), now));
        assert!(!due(Some("2026-10-22T16:00:00+05:00"), now));
        assert!(due(Some("2026-10-23T12:00:00Z"), now));
    }

    #[tokio::test]
    async fn one_operation_owns_the_pending_update_at_a_time() {
        let updates = Updates::default();
        let guard = updates.pending.try_lock().unwrap();
        assert!(updates.pending.try_lock().is_err());
        assert!(matches!(updates.status().unwrap().state, UpdatePhase::Idle));
        drop(guard);
        assert!(updates.pending.try_lock().is_ok());
    }
}
