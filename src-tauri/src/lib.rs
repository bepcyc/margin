pub mod cli;
mod commands;
pub mod core;
mod make_active;
mod runtime;
mod updates;

use crate::core::{poller, tray};
use runtime::Runtime;
use std::sync::Arc;
#[cfg(not(target_os = "linux"))]
use std::time::Instant;
use tauri::{Manager, WindowEvent};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri_plugin_notification::NotificationExt;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            tray::show(app, false)
        }))
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec!["--background"]),
        ))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(tray::Visibility::default())
        .manage(updates::Updates::default())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::get_account_history,
            commands::get_popover_placement,
            commands::fit_popover,
            commands::refresh_accounts,
            commands::discover_accounts,
            commands::add_account,
            commands::remove_account,
            commands::restore_ignored_accounts,
            commands::rename_account,
            commands::save_settings,
            commands::reset_store,
            commands::hide_window,
            make_active::preview_make_active,
            make_active::make_active,
            make_active::restore_switch_backup,
            make_active::dismiss_switch_restore,
            updates::get_update_status,
            updates::check_for_update,
            updates::install_update,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let path = app.path().app_data_dir()?.join("state.json");
            let runtime = Arc::new(Runtime::new(path).map_err(std::io::Error::other)?);
            app.manage(runtime.clone());
            tray::setup(app.handle())?;
            let opener = app.handle().clone();
            app.notification().on_action(move |performed| {
                if performed.action_id() == "tap" {
                    tray::show(&opener, false);
                }
            })?;
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if let Ok(enabled) = handle.autolaunch().is_enabled() {
                    let mut inner = runtime.inner.lock().await;
                    inner.view.settings.autostart = enabled;
                    inner.view.changed();
                }
                if runtime.first_run {
                    if let Err(error) = commands::discover(&handle, &runtime, true).await {
                        let mut inner = runtime.inner.lock().await;
                        inner.view.notice = Some(error);
                        inner.view.changed();
                    }
                }
                let view = runtime.inner.lock().await.view.clone();
                runtime::publish(&handle, &view);
                #[cfg(target_os = "macos")]
                poller::watch_wake(runtime.clone());
                updates::start_background_checks(handle.clone(), runtime.clone());
                poller::start(handle.clone(), runtime);
                if !std::env::args().any(|argument| argument == "--background") {
                    tray::show(&handle, false);
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::Focused(false) => {
                #[cfg(target_os = "linux")]
                {
                    let generation = window
                        .app_handle()
                        .state::<tray::Visibility>()
                        .blur_generation
                        .next();
                    let window = window.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                        let app = window.app_handle().clone();
                        let _ = app.run_on_main_thread(move || {
                            if window
                                .app_handle()
                                .state::<tray::Visibility>()
                                .blur_generation
                                .current(generation)
                                && matches!(window.is_focused(), Ok(false))
                                && window.hide().is_ok()
                            {
                                tray::mark_hidden(window.app_handle());
                            }
                        });
                    });
                }
                #[cfg(not(target_os = "linux"))]
                {
                    if let Ok(mut last_blur) = window
                        .app_handle()
                        .state::<tray::Visibility>()
                        .last_blur
                        .lock()
                    {
                        *last_blur = Some(Instant::now());
                    }
                    if window.hide().is_ok() {
                        tray::mark_hidden(window.app_handle());
                    }
                }
            }
            #[cfg(target_os = "linux")]
            WindowEvent::Focused(true) => {
                window
                    .app_handle()
                    .state::<tray::Visibility>()
                    .blur_generation
                    .next();
            }
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                if window.hide().is_ok() {
                    tray::mark_hidden(window.app_handle());
                }
            }
            _ => {}
        })
        .build(tauri::generate_context!())
        .expect("Margin could not start")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                flush_on_exit(app);
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                tray::show(app, false);
            }
        });
}

/// Readings are saved every 30 seconds; write the latest one before quitting.
fn flush_on_exit(app: &tauri::AppHandle) {
    let Some(runtime) = app.try_state::<Arc<Runtime>>() else {
        return;
    };
    if !runtime.has_pending_save() {
        return;
    }
    let inner = tauri::async_runtime::block_on(runtime.inner.lock());
    let _ = runtime.persist(&inner.view, &inner.alerts);
}
