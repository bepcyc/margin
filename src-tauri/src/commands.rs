use crate::{
    core::{
        accounts,
        digest::DigestState,
        model::{Account, Provider},
        poller,
        settings::{OverviewLayout, Settings, TrayMode},
    },
    runtime::{publish, AppState, Runtime},
};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

#[tauri::command]
pub async fn get_account_history(
    runtime: State<'_, Arc<Runtime>>,
    id: String,
) -> Result<crate::core::history::AccountHistory, String> {
    let mut inner = runtime.inner.lock().await;
    let account = inner
        .view
        .accounts
        .iter()
        .find(|account| account.id == id)
        .cloned()
        .ok_or("Account no longer exists.")?;
    Ok(inner.history.get(&account, chrono::Utc::now()))
}

#[tauri::command]
pub async fn get_state(runtime: State<'_, Arc<Runtime>>) -> Result<AppState, String> {
    Ok(runtime.inner.lock().await.view.clone())
}

#[tauri::command]
pub fn get_popover_placement(
    visibility: State<'_, crate::core::tray::Visibility>,
) -> Result<crate::core::tray::PopoverPlacement, String> {
    visibility
        .placement
        .lock()
        .map(|placement| placement.clone())
        .map_err(|_| "Popover position is unavailable.".into())
}

#[tauri::command]
pub fn fit_popover(app: AppHandle, height: f64) -> Result<(), String> {
    crate::core::tray::fit(&app, height)
}

#[tauri::command]
pub async fn refresh_accounts(runtime: State<'_, Arc<Runtime>>) -> Result<(), String> {
    poller::refresh(&runtime).await;
    Ok(())
}

#[tauri::command]
pub async fn discover_accounts(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
) -> Result<AppState, String> {
    discover(&app, &runtime, false).await
}

pub async fn discover(
    app: &AppHandle,
    runtime: &Runtime,
    initial: bool,
) -> Result<AppState, String> {
    let (existing, ignored) = {
        let inner = runtime.inner.lock().await;
        (
            inner.view.accounts.clone(),
            inner.view.ignored_sources.clone(),
        )
    };
    let starting_ids: Vec<String> = existing.iter().map(|account| account.id.clone()).collect();
    let discovered =
        tauri::async_runtime::spawn_blocking(move || accounts::discover(&existing, &ignored))
            .await
            .map_err(|_| "Account discovery could not finish.")?;
    runtime
        .change(app, move |state| {
            merge_discovered(state, discovered, &starting_ids, initial);
            Ok(())
        })
        .await
}

fn merge_discovered(
    state: &mut AppState,
    discovered: Vec<Account>,
    starting_ids: &[String],
    initial: bool,
) {
    for account in discovered {
        if let Some(existing) = state.accounts.iter_mut().find(|existing| {
            existing.id == account.id
                && accounts::gains_keychain_link(&existing.source, &account.source)
        }) {
            existing.source = account.source;
            continue;
        }
        if state.accounts.len() >= 100 {
            break;
        }
        if !starting_ids.contains(&account.id)
            && !state
                .accounts
                .iter()
                .any(|existing| existing.id == account.id)
        {
            state.accounts.push(account);
        }
    }
    if initial && state.accounts.len() >= 3 {
        state.settings.overview_layout = OverviewLayout::Compact;
    }
}

fn remove(state: &mut AppState, id: &str) -> Result<(), String> {
    let removed = state
        .accounts
        .iter()
        .find(|account| account.id == id)
        .ok_or("Account no longer exists.")?;
    accounts::ignore_source(&mut state.ignored_sources, removed);
    state.accounts.retain(|account| account.id != id);
    if state.settings.pinned_account_id.as_deref() == Some(id) {
        state.settings.pinned_account_id = None;
        state.settings.tray_mode = TrayMode::Lowest;
    }
    Ok(())
}

#[tauri::command]
pub async fn add_account(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
    provider: Provider,
    path: String,
) -> Result<AppState, String> {
    if path.len() > 4096 {
        return Err("That folder path is too long.".into());
    }
    let account =
        tauri::async_runtime::spawn_blocking(move || accounts::add_config_dir(provider, &path))
            .await
            .map_err(|_| "Could not inspect that folder.")??;
    runtime
        .change(&app, move |state| {
            accounts::unignore_source(&mut state.ignored_sources, &account);
            if accounts::link_keychain_item(&mut state.accounts, &account) {
                return Ok(());
            }
            if state.accounts.iter().any(|item| item.id == account.id) {
                return Err("That account folder is already connected.".into());
            }
            if state.accounts.len() >= 100 {
                return Err("Margin supports up to 100 account folders.".into());
            }
            state.accounts.push(account);
            Ok(())
        })
        .await
}

#[tauri::command]
pub async fn remove_account(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
    id: String,
) -> Result<AppState, String> {
    runtime.change(&app, move |state| remove(state, &id)).await
}

#[tauri::command]
pub async fn restore_ignored_accounts(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
) -> Result<AppState, String> {
    runtime
        .change(&app, |state| {
            state.ignored_sources.clear();
            Ok(())
        })
        .await?;
    discover(&app, &runtime, false).await
}

#[tauri::command]
pub async fn rename_account(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
    id: String,
    label: String,
) -> Result<AppState, String> {
    let label = label.trim().to_string();
    if label.is_empty() || label.chars().count() > 48 || label.chars().any(char::is_control) {
        return Err("Use an account name between 1 and 48 characters.".into());
    }
    runtime
        .change(&app, move |state| {
            let account = state
                .accounts
                .iter_mut()
                .find(|account| account.id == id)
                .ok_or("Account no longer exists.")?;
            account.label = label;
            Ok(())
        })
        .await
}

#[tauri::command]
pub async fn save_settings(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
    settings: Settings,
) -> Result<AppState, String> {
    settings.validate()?;
    runtime.ensure_store_available()?;
    let mut inner = runtime.inner.lock().await;
    if settings
        .pinned_account_id
        .as_ref()
        .is_some_and(|id| !inner.view.accounts.iter().any(|account| &account.id == id))
    {
        return Err("Choose an account that is still connected.".into());
    }
    let previous_autostart = if inner.view.settings.autostart != settings.autostart {
        app.autolaunch()
            .is_enabled()
            .map_err(|_| "Could not read launch-at-login status.")?
    } else {
        settings.autostart
    };
    if previous_autostart != settings.autostart {
        let result = if settings.autostart {
            app.autolaunch().enable()
        } else {
            app.autolaunch().disable()
        };
        result.map_err(|_| {
            "Could not update launch at login. Try again from an installed copy of Margin."
        })?;
    }
    let mut next = inner.view.clone();
    next.settings = Settings {
        notification_permission: inner.view.settings.notification_permission,
        ..settings
    };
    next.notice = None;
    next.changed();
    let previous_digest = runtime.digest_state();
    let enabling_digest = next.settings.weekly_digest && !inner.view.settings.weekly_digest;
    if enabling_digest {
        runtime.set_digest_state(Some(DigestState::re_enabled(
            previous_digest.clone(),
            chrono::Utc::now(),
        )));
    }
    if let Err(error) = runtime.persist(&next, &inner.alerts) {
        if enabling_digest {
            runtime.set_digest_state(previous_digest);
        }
        if previous_autostart != next.settings.autostart {
            let _ = if previous_autostart {
                app.autolaunch().enable()
            } else {
                app.autolaunch().disable()
            };
        }
        return Err(error);
    }
    inner.view = next.clone();
    drop(inner);
    publish(&app, &next);
    poller::refresh(&runtime).await;
    Ok(next)
}

#[tauri::command]
pub async fn reset_store(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
) -> Result<AppState, String> {
    let reset = runtime.reset_store().await?;
    publish(&app, &reset);
    match discover(&app, &runtime, false).await {
        Ok(next) => Ok(next),
        Err(error) => {
            let mut inner = runtime.inner.lock().await;
            inner.view.notice = Some(error);
            inner.view.changed();
            let next = inner.view.clone();
            drop(inner);
            publish(&app, &next);
            Ok(next)
        }
    }
}

#[tauri::command]
pub fn hide_window(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    if app.state::<crate::gnome_bridge::Bridge>().degraded() {
        app.exit(0);
        return Ok(());
    }
    app.get_webview_window("main")
        .ok_or("Window is unavailable.")?
        .hide()
        .map_err(|_| "Could not close the popover.")?;
    crate::core::tray::mark_hidden(&app);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{AccountSource, Identity};

    fn account(id: &str) -> Account {
        Account {
            id: id.into(),
            provider: Provider::Codex,
            label: id.into(),
            color: "#ffffff".into(),
            source: AccountSource::ConfigDir {
                path: std::env::temp_dir()
                    .join(format!(".codex-{id}"))
                    .to_string_lossy()
                    .into_owned(),
                keychain_suffix: None,
            },
            identity: Identity::default(),
            snapshot: None,
            error: None,
            refreshing: false,
        }
    }

    fn empty() -> AppState {
        AppState {
            revision: 0,
            accounts: Vec::new(),
            settings: Settings::default(),
            refreshing: false,
            notice: None,
            bridge_notice: None,
            recovery: false,
            recovered: Vec::new(),
            ignored_sources: Vec::new(),
            switching: Default::default(),
        }
    }

    #[test]
    fn discovery_follows_the_lowest_account_without_pinning() {
        let mut state = empty();
        merge_discovered(&mut state, vec![account("a"), account("b")], &[], true);
        assert_eq!(state.accounts.len(), 2);
        assert_eq!(state.settings.tray_mode, TrayMode::Lowest);
        assert_eq!(state.settings.pinned_account_id, None);
        assert_eq!(state.settings.overview_layout, OverviewLayout::Cards);
    }

    #[test]
    fn first_discovery_of_many_accounts_starts_compact_once() {
        let mut state = empty();
        let found = vec![account("a"), account("b"), account("c")];
        merge_discovered(&mut state, found.clone(), &[], true);
        assert_eq!(state.settings.overview_layout, OverviewLayout::Compact);
        state.settings.overview_layout = OverviewLayout::Cards;
        merge_discovered(&mut state, found.clone(), &[], false);
        assert_eq!(state.settings.overview_layout, OverviewLayout::Cards);
        let mut later = empty();
        merge_discovered(&mut later, found, &[], false);
        assert_eq!(later.settings.overview_layout, OverviewLayout::Cards);
    }

    #[test]
    fn removing_the_pinned_account_clears_the_pin() {
        let mut state = empty();
        merge_discovered(&mut state, vec![account("a"), account("b")], &[], false);
        state.settings.tray_mode = TrayMode::Pinned;
        state.settings.pinned_account_id = Some("a".into());
        remove(&mut state, "b").unwrap();
        assert_eq!(state.settings.pinned_account_id.as_deref(), Some("a"));
        remove(&mut state, "a").unwrap();
        assert_eq!(state.settings.pinned_account_id, None);
        assert_eq!(state.settings.tray_mode, TrayMode::Lowest);
        assert!(remove(&mut state, "a").is_err());
    }
}
