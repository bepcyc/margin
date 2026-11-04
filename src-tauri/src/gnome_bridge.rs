use gio::{DBusCallFlags, DBusConnection, DBusConnectionFlags, DBusSignalFlags};
use glib::prelude::ObjectExt;
use glib::variant::ToVariant;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager};
use tokio::sync::Notify;

const NAME: &str = "io.github.bepcyc.MarginBridge";
const PATH: &str = "/io/github/bepcyc/MarginBridge";
const INTERFACE: &str = "io.github.bepcyc.MarginBridge";
const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Phase {
    #[default]
    Discovering,
    Registering,
    CreatingFallback,
    Active,
    Fallback,
    Degraded,
}

#[derive(Default)]
pub struct Bridge {
    phase: Mutex<Phase>,
    connection: Mutex<Option<Lease>>,
    updates: Mutex<Updates>,
    ready: Notify,
}

#[derive(Default)]
struct Updates {
    pending: Option<glib::Variant>,
    running: bool,
    failures: u8,
}

impl Updates {
    fn push(&mut self, params: glib::Variant) -> bool {
        self.pending = Some(params);
        if self.running {
            false
        } else {
            self.running = true;
            true
        }
    }

    fn success(&mut self) -> Option<glib::Variant> {
        self.failures = 0;
        let next = self.pending.take();
        self.running = next.is_some();
        next
    }

    fn failure(&mut self) -> bool {
        self.failures += 1;
        self.failures >= 3
    }
}

struct Lease {
    connection: DBusConnection,
    owner: String,
    action_signal: gio::SignalSubscriptionId,
    owner_signal: gio::SignalSubscriptionId,
    closed_signal: glib::SignalHandlerId,
}

impl Bridge {
    fn transition(&self, from: Phase, to: Phase) -> bool {
        let Ok(mut phase) = self.phase.lock() else {
            return false;
        };
        if *phase != from {
            return false;
        }
        *phase = to;
        if matches!(to, Phase::Active | Phase::Fallback | Phase::Degraded) {
            self.ready.notify_waiters();
        }
        true
    }

    pub fn active(&self) -> bool {
        self.phase.lock().is_ok_and(|phase| *phase == Phase::Active)
    }

    pub fn degraded(&self) -> bool {
        self.phase
            .lock()
            .is_ok_and(|phase| *phase == Phase::Degraded)
    }

    pub async fn wait_for_backend(&self) -> bool {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            let phase = self.phase.lock().ok().map(|phase| *phase);
            match phase {
                Some(Phase::Active | Phase::Fallback) => return true,
                Some(Phase::Degraded) | None => return false,
                _ => ready.await,
            }
        }
    }
}

async fn call(
    connection: &DBusConnection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    parameters: &glib::Variant,
) -> Result<glib::Variant, glib::Error> {
    connection
        .call_future(
            Some(destination),
            path,
            interface,
            method,
            Some(parameters),
            None,
            DBusCallFlags::NONE,
            1500,
        )
        .await
}

async fn owner(connection: &DBusConnection, name: &str) -> Option<String> {
    call(
        connection,
        BUS,
        BUS_PATH,
        BUS,
        "GetNameOwner",
        &(name,).to_variant(),
    )
    .await
    .ok()?
    .get::<(String,)>()
    .map(|v| v.0)
}

fn degraded(app: &AppHandle) {
    let bridge = app.state::<Bridge>();
    if !(bridge.transition(Phase::Registering, Phase::Degraded)
        || bridge.transition(Phase::Active, Phase::Degraded)
        || bridge.transition(Phase::Discovering, Phase::Degraded)
        || bridge.transition(Phase::CreatingFallback, Phase::Degraded)
        || bridge.transition(Phase::Fallback, Phase::Degraded))
    {
        return;
    }
    if let Some(lease) = bridge
        .connection
        .lock()
        .ok()
        .and_then(|mut current| current.take())
    {
        lease.connection.signal_unsubscribe(lease.action_signal);
        lease.connection.signal_unsubscribe(lease.owner_signal);
        lease.connection.disconnect(lease.closed_signal);
        glib::MainContext::default().spawn_local(async move {
            let _ = lease.connection.close_future().await;
        });
    }
    let handle = app.clone();
    let runtime = app.state::<Arc<crate::runtime::Runtime>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let view = {
            let mut inner = runtime.inner.lock().await;
            inner.view.bridge_notice = Some("The GNOME panel connection is unavailable. Close and restart Margin to restore the tray icon.".into());
            inner.view.changed();
            inner.view.clone()
        };
        crate::runtime::publish(&handle, &view);
        crate::core::tray::show(&handle, false);
    });
}

fn fallback(app: &AppHandle) {
    if app
        .state::<Bridge>()
        .transition(Phase::Discovering, Phase::CreatingFallback)
    {
        if crate::core::tray::setup_native(app).is_err() {
            degraded(app);
            return;
        }
        if !app
            .state::<Bridge>()
            .transition(Phase::CreatingFallback, Phase::Fallback)
        {
            return;
        }
        let runtime = app.state::<Arc<crate::runtime::Runtime>>().inner().clone();
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            let view = runtime.inner.lock().await.view.clone();
            crate::runtime::publish(&handle, &view);
        });
    }
}

pub fn start(app: &AppHandle) {
    let handle = app.clone();
    glib::MainContext::default().spawn_local(async move {
        let Ok(observer) = gio::bus_get_future(gio::BusType::Session).await else {
            degraded(&handle);
            return;
        };
        let Ok(has_owner) = call(
            &observer,
            BUS,
            BUS_PATH,
            BUS,
            "NameHasOwner",
            &(NAME,).to_variant(),
        )
        .await
        else {
            degraded(&handle);
            return;
        };
        if has_owner.get::<(bool,)>() == Some((false,)) {
            fallback(&handle);
            return;
        }
        let (Some(shell), Some(bridge_owner)) = (
            owner(&observer, "org.gnome.Shell").await,
            owner(&observer, NAME).await,
        ) else {
            degraded(&handle);
            return;
        };
        if shell != bridge_owner {
            degraded(&handle);
            return;
        }
        let Ok(address) = std::env::var("DBUS_SESSION_BUS_ADDRESS") else {
            degraded(&handle);
            return;
        };
        let Ok(connection) = DBusConnection::for_address_future(
            &address,
            DBusConnectionFlags::AUTHENTICATION_CLIENT
                | DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
            None,
        )
        .await
        else {
            degraded(&handle);
            return;
        };
        let signal_app = handle.clone();
        let action_signal = connection.signal_subscribe(
            Some(&shell),
            Some(INTERFACE),
            Some("Action"),
            Some(PATH),
            None,
            DBusSignalFlags::NONE,
            move |_, _, _, _, _, args| {
                if !signal_app.state::<Bridge>().active() {
                    return;
                }
                if let Some((action,)) = args.get::<(String,)>() {
                    match action.as_str() {
                        "open" => crate::core::tray::show(&signal_app, false),
                        "toggle" => crate::core::tray::toggle_from_bridge(&signal_app),
                        "settings" => crate::core::tray::show(&signal_app, true),
                        "refresh" => {
                            let runtime = signal_app
                                .state::<Arc<crate::runtime::Runtime>>()
                                .inner()
                                .clone();
                            tauri::async_runtime::spawn(async move {
                                crate::core::poller::refresh(&runtime).await;
                            });
                        }
                        "quit" => signal_app.exit(0),
                        _ => {}
                    }
                }
            },
        );
        let changed_app = handle.clone();
        let watched_owner = shell.clone();
        let owner_signal = connection.signal_subscribe(
            Some(BUS),
            Some(BUS),
            Some("NameOwnerChanged"),
            Some(BUS_PATH),
            None,
            DBusSignalFlags::NONE,
            move |_, _, _, _, _, args| {
                if let Some((name, old, new)) = args.get::<(String, String, String)>() {
                    if (name == NAME || name == "org.gnome.Shell")
                        && old == watched_owner
                        && new != watched_owner
                    {
                        degraded(&changed_app);
                    }
                }
            },
        );
        let closed_app = handle.clone();
        let closed_signal = connection.connect_closed(move |_, _, _| degraded(&closed_app));
        let bridge = handle.state::<Bridge>();
        if let Ok(mut current) = bridge.connection.lock() {
            *current = Some(Lease {
                connection: connection.clone(),
                owner: shell.clone(),
                action_signal,
                owner_signal,
                closed_signal,
            });
        } else {
            degraded(&handle);
            return;
        }
        if !handle
            .state::<Bridge>()
            .transition(Phase::Discovering, Phase::Registering)
        {
            return;
        }
        if owner(&connection, NAME).await.as_deref() != Some(&shell)
            || owner(&connection, "org.gnome.Shell").await.as_deref() != Some(&shell)
        {
            degraded(&handle);
            return;
        }
        let (size, pixels, tooltip) = crate::core::tray::initial_bridge_icon(&handle);
        let parameters = (size, pixels, tooltip, 0_u64).to_variant();
        let response = call(
            &connection,
            &shell,
            PATH,
            INTERFACE,
            "Register",
            &parameters,
        )
        .await;
        if response.ok().and_then(|v| v.get::<(bool,)>()) != Some((true,)) {
            degraded(&handle);
            return;
        }
        if !bridge.transition(Phase::Registering, Phase::Active) {
            return;
        }
        let runtime = handle
            .state::<Arc<crate::runtime::Runtime>>()
            .inner()
            .clone();
        let app = handle.clone();
        tauri::async_runtime::spawn(async move {
            let view = runtime.inner.lock().await.view.clone();
            crate::runtime::publish(&app, &view);
        });
    });
}

pub fn update(app: &AppHandle, tooltip: &str, size: u32, pixels: &[u8], revision: u64) {
    let bridge = app.state::<Bridge>();
    if !bridge.active() {
        return;
    }
    let Ok(current) = bridge.connection.lock() else {
        return;
    };
    let Some(lease) = current.as_ref() else {
        return;
    };
    let connection = lease.connection.clone();
    let owner = lease.owner.clone();
    drop(current);
    let app = app.clone();
    let params = (size, pixels, tooltip, revision).to_variant();
    let Ok(mut updates) = bridge.updates.lock() else {
        return;
    };
    if !updates.push(params) {
        return;
    }
    drop(updates);
    glib::MainContext::default().spawn_local(async move {
        let bridge = app.state::<Bridge>();
        let mut params = match bridge
            .updates
            .lock()
            .ok()
            .and_then(|mut u| u.pending.take())
        {
            Some(params) => params,
            None => return,
        };
        loop {
            if !bridge.active() {
                return;
            }
            let response = call(&connection, &owner, PATH, INTERFACE, "Update", &params).await;
            if !bridge.active() {
                return;
            }
            let Ok(mut updates) = bridge.updates.lock() else {
                degraded(&app);
                return;
            };
            if response.as_ref().ok().and_then(|v| v.get::<(bool,)>()) == Some((true,)) {
                match updates.success() {
                    Some(next) => params = next,
                    None => return,
                }
            } else {
                eprintln!("GNOME bridge Update failed: {response:?}");
                if updates.failure() {
                    degraded(&app);
                    return;
                }
                if let Some(next) = updates.pending.take() {
                    params = next;
                }
            }
        }
    });
}

pub fn reposition(app: &AppHandle) {
    let bridge = app.state::<Bridge>();
    let Ok(current) = bridge.connection.lock() else {
        return;
    };
    let Some(lease) = current.as_ref() else {
        return;
    };
    let connection = lease.connection.clone();
    let owner = lease.owner.clone();
    glib::MainContext::default().spawn_local(async move {
        let _ = call(
            &connection,
            &owner,
            PATH,
            INTERFACE,
            "Reposition",
            &().to_variant(),
        )
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::{Bridge, Phase, Updates};
    use glib::variant::ToVariant;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn update_coalesces_and_keeps_latest_after_success() {
        let mut updates = Updates::default();
        assert!(updates.push(1_u64.to_variant()));
        assert_eq!(updates.pending.take().unwrap().get::<u64>(), Some(1));
        assert!(!updates.push(2_u64.to_variant()));
        assert!(!updates.push(3_u64.to_variant()));
        assert_eq!(updates.success().unwrap().get::<u64>(), Some(3));
        assert!(updates.running);
        assert!(updates.success().is_none());
        assert!(!updates.running);
    }

    #[test]
    fn continuous_failures_are_bounded_despite_new_updates() {
        let mut updates = Updates::default();
        assert!(updates.push(1_u64.to_variant()));
        updates.pending.take();
        assert!(!updates.failure());
        assert!(!updates.push(2_u64.to_variant()));
        assert!(!updates.failure());
        assert!(!updates.push(3_u64.to_variant()));
        assert!(updates.failure());
    }

    #[test]
    fn transient_failure_then_success_resets_budget() {
        let mut updates = Updates::default();
        assert!(updates.push(1_u64.to_variant()));
        updates.pending.take();
        assert!(!updates.failure());
        assert!(updates.success().is_none());
        assert_eq!(updates.failures, 0);
        assert!(updates.push(2_u64.to_variant()));
        assert!(!updates.failure());
    }

    #[test]
    fn late_registration_cannot_replace_degraded_state() {
        let bridge = Bridge::default();
        assert!(bridge.transition(Phase::Discovering, Phase::Registering));
        assert!(bridge.transition(Phase::Registering, Phase::Degraded));
        assert!(!bridge.transition(Phase::Registering, Phase::Active));
        assert!(!bridge.transition(Phase::Discovering, Phase::Fallback));
    }

    #[test]
    fn fallback_cannot_register_later() {
        let bridge = Bridge::default();
        assert!(bridge.transition(Phase::Discovering, Phase::CreatingFallback));
        assert!(bridge.transition(Phase::CreatingFallback, Phase::Fallback));
        assert!(!bridge.transition(Phase::Discovering, Phase::Registering));
        assert!(bridge.transition(Phase::Fallback, Phase::Degraded));
        assert!(!bridge.transition(Phase::Discovering, Phase::Fallback));
    }

    #[tokio::test]
    async fn initial_show_waits_for_registration() {
        let bridge = Arc::new(Bridge::default());
        assert!(bridge.transition(Phase::Discovering, Phase::Registering));
        let waiting = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.wait_for_backend().await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert!(bridge.transition(Phase::Registering, Phase::Active));
        assert!(tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap());
    }

    #[tokio::test]
    async fn initial_show_does_not_duplicate_degraded_recovery() {
        let bridge = Bridge::default();
        assert!(bridge.transition(Phase::Discovering, Phase::Registering));
        assert!(bridge.transition(Phase::Registering, Phase::Degraded));
        assert!(!bridge.wait_for_backend().await);
    }

    #[tokio::test]
    async fn all_show_requests_wait_for_fallback_creation() {
        let bridge = Arc::new(Bridge::default());
        assert!(bridge.transition(Phase::Discovering, Phase::CreatingFallback));
        let mut waiting = Vec::new();
        for _ in 0..3 {
            let bridge = bridge.clone();
            waiting.push(tokio::spawn(async move { bridge.wait_for_backend().await }));
        }
        tokio::task::yield_now().await;
        assert!(waiting.iter().all(|task| !task.is_finished()));
        assert!(bridge.transition(Phase::CreatingFallback, Phase::Fallback));
        for task in waiting {
            assert!(tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap());
        }
    }

    #[tokio::test]
    async fn failed_fallback_does_not_release_show_as_ready() {
        let bridge = Arc::new(Bridge::default());
        assert!(bridge.transition(Phase::Discovering, Phase::CreatingFallback));
        let waiting = tokio::spawn({
            let bridge = bridge.clone();
            async move { bridge.wait_for_backend().await }
        });
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        assert!(bridge.transition(Phase::CreatingFallback, Phase::Degraded));
        assert!(!tokio::time::timeout(Duration::from_secs(1), waiting)
            .await
            .unwrap()
            .unwrap());
    }
}
