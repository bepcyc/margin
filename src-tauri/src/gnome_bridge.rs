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
    Recovering,
    Fallback,
    Degraded,
}

#[derive(Default)]
pub struct Bridge {
    phase: Mutex<Lifecycle>,
    ready: Notify,
}

#[derive(Default)]
struct Lifecycle {
    phase: Phase,
    generation: u64,
    lease: Option<Lease>,
    observer: Option<DBusConnection>,
    updates: Updates,
    deadline: Option<std::time::Instant>,
}

impl Lifecycle {
    fn updates_for(&mut self, generation: u64) -> Option<&mut Updates> {
        (self.generation == generation
            && self.phase == Phase::Active
            && self.updates.generation == generation)
            .then_some(&mut self.updates)
    }

    fn outage_deadline(&mut self, now: std::time::Instant) -> std::time::Instant {
        *self
            .deadline
            .get_or_insert(now + std::time::Duration::from_secs(20))
    }
}

#[derive(Default)]
struct Updates {
    generation: u64,
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
    closed_signal: glib::SignalHandlerId,
}

impl Bridge {
    fn transition(&self, from: Phase, to: Phase) -> bool {
        let Ok(mut state) = self.phase.lock() else {
            return false;
        };
        if state.phase != from {
            return false;
        }
        state.phase = to;
        if matches!(to, Phase::Active | Phase::Fallback | Phase::Degraded) {
            self.ready.notify_waiters();
        }
        true
    }

    fn generation(&self) -> u64 {
        self.phase.lock().map_or(u64::MAX, |state| state.generation)
    }

    fn current(&self, generation: u64, phase: Phase) -> bool {
        self.phase
            .lock()
            .is_ok_and(|state| state.generation == generation && state.phase == phase)
    }

    pub fn active(&self) -> bool {
        self.phase
            .lock()
            .is_ok_and(|state| state.phase == Phase::Active)
    }

    pub fn is_active_generation(&self, generation: u64) -> bool {
        self.current(generation, Phase::Active)
    }

    pub fn retains_position(&self) -> bool {
        self.phase.lock().is_ok_and(|state| {
            state.phase == Phase::Active
                || state.phase == Phase::Recovering
                || (state.phase == Phase::Registering && state.deadline.is_some())
        })
    }

    pub fn degraded(&self) -> bool {
        self.phase.lock().is_ok_and(|state| {
            matches!(state.phase, Phase::Recovering | Phase::Degraded)
                || (state.phase == Phase::Registering && state.deadline.is_some())
        })
    }

    pub async fn wait_for_backend(&self) -> bool {
        loop {
            let ready = self.ready.notified();
            tokio::pin!(ready);
            ready.as_mut().enable();
            let phase = self.phase.lock().ok().map(|state| state.phase);
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

async fn close_lease(lease: Lease) {
    lease.connection.signal_unsubscribe(lease.action_signal);
    lease.connection.disconnect(lease.closed_signal);
    let _ = glib::future_with_timeout(
        std::time::Duration::from_secs(2),
        lease.connection.close_future(),
    )
    .await;
}

fn terminal(app: &AppHandle, generation: u64) {
    let bridge = app.state::<Bridge>();
    let (lease, terminal_generation) = {
        let Ok(mut state) = bridge.phase.lock() else {
            return;
        };
        if state.generation != generation || state.phase == Phase::Degraded {
            return;
        }
        state.generation = state.generation.saturating_add(1);
        state.phase = Phase::Degraded;
        state.updates = Updates::default();
        state.deadline = None;
        bridge.ready.notify_waiters();
        (state.lease.take(), state.generation)
    };
    if let Some(lease) = lease {
        glib::MainContext::default().spawn_local(close_lease(lease));
    }
    let handle = app.clone();
    let runtime = app.state::<Arc<crate::runtime::Runtime>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let view = {
            let mut inner = runtime.inner.lock().await;
            if !handle
                .state::<Bridge>()
                .current(terminal_generation, Phase::Degraded)
            {
                return;
            }
            inner.view.bridge_notice = Some("The GNOME panel connection is unavailable. Close and restart Margin to restore the tray icon.".into());
            inner.view.changed();
            inner.view.clone()
        };
        crate::runtime::publish(&handle, &view);
        if handle
            .state::<Bridge>()
            .current(terminal_generation, Phase::Degraded)
        {
            crate::core::tray::show(&handle, false);
        }
    });
}

fn fallback(app: &AppHandle) {
    if app
        .state::<Bridge>()
        .transition(Phase::Discovering, Phase::CreatingFallback)
    {
        if crate::core::tray::setup_native(app).is_err() {
            terminal(app, app.state::<Bridge>().generation());
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

fn failure(app: &AppHandle, generation: u64) {
    let observer = app.state::<Bridge>().phase.lock().ok().and_then(|state| {
        if state.generation == generation && state.lease.is_some() {
            state.observer.clone()
        } else {
            None
        }
    });
    if let Some(observer) = observer {
        lost(app, generation, observer);
    } else {
        terminal(app, generation);
    }
}

fn lost(app: &AppHandle, generation: u64, observer: DBusConnection) {
    let bridge = app.state::<Bridge>();
    let (lease, next, deadline) = {
        let Ok(mut state) = bridge.phase.lock() else {
            return;
        };
        if state.generation != generation
            || !matches!(state.phase, Phase::Active | Phase::Registering)
        {
            return;
        }
        let Some(lease) = state.lease.take() else {
            return;
        };
        let deadline = state.outage_deadline(std::time::Instant::now());
        state.generation = state.generation.saturating_add(1);
        state.phase = Phase::Recovering;
        state.updates = Updates::default();
        (lease, state.generation, deadline)
    };
    let handle = app.clone();
    let notice_app = app.clone();
    let runtime = app.state::<Arc<crate::runtime::Runtime>>().inner().clone();
    tauri::async_runtime::spawn(async move {
        let view = {
            let mut inner = runtime.inner.lock().await;
            let bridge = notice_app.state::<Bridge>();
            let Ok(state) = bridge.phase.lock() else {
                return;
            };
            if state.generation != next
                || !matches!(state.phase, Phase::Recovering | Phase::Registering)
                || state.deadline.is_none()
            {
                return;
            }
            inner.view.bridge_notice = Some("Reconnecting to the GNOME panel…".into());
            inner.view.changed();
            inner.view.clone()
        };
        let main_app = notice_app.clone();
        let _ = notice_app.run_on_main_thread(move || {
            let bridge = main_app.state::<Bridge>();
            let current = bridge.phase.lock().is_ok_and(|state| {
                state.generation == next
                    && matches!(state.phase, Phase::Recovering | Phase::Registering)
                    && state.deadline.is_some()
            });
            if current {
                crate::runtime::publish(&main_app, &view);
            }
        });
    });
    glib::MainContext::default().spawn_local(async move {
        let old_shell = lease.owner.clone();
        let old_client = lease.connection.unique_name().map(|name| name.to_string());
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if glib::future_with_timeout(remaining, close_lease(lease))
            .await
            .is_err()
        {
            terminal(&handle, next);
            return;
        }
        let Some(old_client) = old_client else {
            terminal(&handle, next);
            return;
        };
        while std::time::Instant::now() < deadline
            && handle.state::<Bridge>().current(next, Phase::Recovering)
        {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let probe = async {
                let absent = call(
                    &observer,
                    BUS,
                    BUS_PATH,
                    BUS,
                    "NameHasOwner",
                    &(old_client.as_str(),).to_variant(),
                )
                .await
                .ok()
                .and_then(|v| v.get::<(bool,)>())
                    == Some((false,));
                if !absent {
                    return None;
                }
                let shell = owner(&observer, "org.gnome.Shell").await?;
                let bridge_owner = owner(&observer, NAME).await?;
                (shell != old_shell && shell == bridge_owner).then_some(shell)
            };
            let Ok(candidate) = glib::future_with_timeout(remaining, probe).await else {
                break;
            };
            if let Some(shell) = candidate {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if glib::future_with_timeout(
                    remaining,
                    register(&handle, observer.clone(), shell, next, true),
                )
                .await
                .is_err()
                {
                    terminal(&handle, next);
                }
                return;
            }
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            glib::timeout_future(remaining.min(std::time::Duration::from_millis(250))).await;
        }
        terminal(&handle, next);
    });
}

async fn register(
    app: &AppHandle,
    observer: DBusConnection,
    shell: String,
    generation: u64,
    recovery: bool,
) {
    let bridge = app.state::<Bridge>();
    {
        let Ok(mut state) = bridge.phase.lock() else {
            return;
        };
        let expected = if recovery {
            Phase::Recovering
        } else {
            Phase::Discovering
        };
        if state.generation != generation || state.phase != expected {
            return;
        }
        state.phase = Phase::Registering;
    }
    let Ok(address) = std::env::var("DBUS_SESSION_BUS_ADDRESS") else {
        failure(app, generation);
        return;
    };
    let result = glib::future_with_timeout(
        std::time::Duration::from_secs(2),
        DBusConnection::for_address_future(
            &address,
            DBusConnectionFlags::AUTHENTICATION_CLIENT
                | DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
            None,
        ),
    )
    .await;
    let Ok(Ok(connection)) = result else {
        failure(app, generation);
        return;
    };
    let action_app = app.clone();
    let action_signal = connection.signal_subscribe(
        Some(&shell),
        Some(INTERFACE),
        Some("Action"),
        Some(PATH),
        None,
        DBusSignalFlags::NONE,
        move |_, _, _, _, _, args| {
            if !action_app
                .state::<Bridge>()
                .current(generation, Phase::Active)
            {
                return;
            }
            if let Some((action,)) = args.get::<(String,)>() {
                match action.as_str() {
                    "open" => crate::core::tray::show_from_bridge(&action_app, false, generation),
                    "toggle" => crate::core::tray::toggle_from_bridge(&action_app, generation),
                    "settings" => {
                        crate::core::tray::show_from_bridge(&action_app, true, generation)
                    }
                    "refresh" => {
                        let runtime = action_app
                            .state::<Arc<crate::runtime::Runtime>>()
                            .inner()
                            .clone();
                        let refresh_app = action_app.clone();
                        tauri::async_runtime::spawn(async move {
                            if refresh_app
                                .state::<Bridge>()
                                .is_active_generation(generation)
                            {
                                crate::core::poller::refresh(&runtime).await;
                            }
                        });
                    }
                    "quit" => action_app.exit(0),
                    _ => {}
                }
            }
        },
    );
    let closed_app = app.clone();
    let closed_observer = observer.clone();
    let closed_signal = connection
        .connect_closed(move |_, _, _| lost(&closed_app, generation, closed_observer.clone()));
    let mut pending_lease = Some(Lease {
        connection: connection.clone(),
        owner: shell.clone(),
        action_signal,
        closed_signal,
    });
    let installed = if let Ok(mut state) = bridge.phase.lock() {
        if state.generation == generation && state.phase == Phase::Registering {
            state.lease = pending_lease.take();
            state.updates = Updates {
                generation,
                ..Updates::default()
            };
            true
        } else {
            false
        }
    } else {
        false
    };
    if !installed {
        if let Some(lease) = pending_lease {
            close_lease(lease).await;
        }
        return;
    }
    if owner(&observer, NAME).await.as_deref() != Some(&shell)
        || owner(&observer, "org.gnome.Shell").await.as_deref() != Some(&shell)
    {
        failure(app, generation);
        return;
    }
    let (size, pixels, tooltip) = crate::core::tray::initial_bridge_icon(app);
    let response = call(
        &connection,
        &shell,
        PATH,
        INTERFACE,
        "Register",
        &(size, pixels, tooltip, 0_u64).to_variant(),
    )
    .await;
    if response.ok().and_then(|v| v.get::<(bool,)>()) != Some((true,)) {
        failure(app, generation);
        return;
    }
    if owner(&observer, NAME).await.as_deref() != Some(&shell)
        || owner(&observer, "org.gnome.Shell").await.as_deref() != Some(&shell)
    {
        failure(app, generation);
        return;
    }
    {
        let Ok(mut state) = bridge.phase.lock() else {
            return;
        };
        if state.generation != generation
            || state.phase != Phase::Registering
            || state
                .lease
                .as_ref()
                .is_none_or(|lease| lease.owner != shell)
        {
            return;
        }
        if state
            .deadline
            .is_some_and(|deadline| std::time::Instant::now() >= deadline)
        {
            drop(state);
            terminal(app, generation);
            return;
        }
        state.phase = Phase::Active;
        state.deadline = None;
        bridge.ready.notify_waiters();
    }
    let runtime = app.state::<Arc<crate::runtime::Runtime>>().inner().clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let view = {
            let mut inner = runtime.inner.lock().await;
            let bridge = handle.state::<Bridge>();
            let Ok(state) = bridge.phase.lock() else {
                return;
            };
            if state.generation != generation || state.phase != Phase::Active {
                return;
            }
            if inner.view.bridge_notice.take().is_some() {
                inner.view.changed();
            }
            inner.view.clone()
        };
        let main_handle = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            if !main_handle
                .state::<Bridge>()
                .current(generation, Phase::Active)
            {
                return;
            }
            crate::runtime::publish(&main_handle, &view);
            if recovery
                && main_handle
                    .get_webview_window("main")
                    .is_some_and(|window| window.is_visible().unwrap_or(false))
            {
                reposition(&main_handle);
            }
        });
    });
}

pub fn start(app: &AppHandle) {
    let handle = app.clone();
    glib::MainContext::default().spawn_local(async move {
        let generation = handle.state::<Bridge>().generation();
        let Ok(observer) = gio::bus_get_future(gio::BusType::Session).await else {
            terminal(&handle, generation);
            return;
        };
        if let Ok(mut state) = handle.state::<Bridge>().phase.lock() {
            if state.generation != generation || state.phase != Phase::Discovering {
                return;
            }
            state.observer = Some(observer.clone());
        } else {
            terminal(&handle, generation);
            return;
        }
        let observed_app = handle.clone();
        let observed_connection = observer.clone();
        observer.signal_subscribe(
            Some(BUS),
            Some(BUS),
            Some("NameOwnerChanged"),
            Some(BUS_PATH),
            None,
            DBusSignalFlags::NONE,
            move |_, _, _, _, _, args| {
                let Some((name, old, new)) = args.get::<(String, String, String)>() else {
                    return;
                };
                if name != NAME && name != "org.gnome.Shell" {
                    return;
                }
                let bridge = observed_app.state::<Bridge>();
                let generation = bridge.phase.lock().ok().and_then(|state| {
                    state
                        .lease
                        .as_ref()
                        .filter(|lease| lease.owner == old && new != old)
                        .map(|_| state.generation)
                });
                if let Some(generation) = generation {
                    lost(&observed_app, generation, observed_connection.clone());
                }
            },
        );
        let has_owner = call(
            &observer,
            BUS,
            BUS_PATH,
            BUS,
            "NameHasOwner",
            &(NAME,).to_variant(),
        )
        .await;
        match has_owner.ok().and_then(|v| v.get::<(bool,)>()) {
            Some((false,)) => {
                fallback(&handle);
                return;
            }
            Some((true,)) => {}
            _ => {
                terminal(&handle, generation);
                return;
            }
        }
        let (Some(shell), Some(bridge_owner)) = (
            owner(&observer, "org.gnome.Shell").await,
            owner(&observer, NAME).await,
        ) else {
            terminal(&handle, generation);
            return;
        };
        if shell != bridge_owner {
            terminal(&handle, generation);
            return;
        }
        register(&handle, observer, shell, generation, false).await;
    });
}

pub fn update(app: &AppHandle, tooltip: &str, size: u32, pixels: &[u8], revision: u64) {
    let bridge = app.state::<Bridge>();
    let params = (size, pixels, tooltip, revision).to_variant();
    let (generation, connection, owner) = {
        let Ok(mut state) = bridge.phase.lock() else {
            return;
        };
        if state.phase != Phase::Active {
            return;
        }
        let Some(lease) = state.lease.as_ref() else {
            return;
        };
        let connection = lease.connection.clone();
        let owner = lease.owner.clone();
        let generation = state.generation;
        if !state.updates.push(params) {
            return;
        }
        (generation, connection, owner)
    };
    let app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        let bridge = app.state::<Bridge>();
        let mut params = {
            let Ok(mut state) = bridge.phase.lock() else {
                return;
            };
            let Some(updates) = state.updates_for(generation) else {
                return;
            };
            let Some(params) = updates.pending.take() else {
                return;
            };
            params
        };
        loop {
            let response = call(&connection, &owner, PATH, INTERFACE, "Update", &params).await;
            let failed;
            {
                let Ok(mut state) = bridge.phase.lock() else {
                    return;
                };
                let Some(updates) = state.updates_for(generation) else {
                    return;
                };
                if response.as_ref().ok().and_then(|v| v.get::<(bool,)>()) == Some((true,)) {
                    match updates.success() {
                        Some(next) => params = next,
                        None => return,
                    }
                    failed = false;
                } else {
                    eprintln!("GNOME bridge Update failed: {response:?}");
                    failed = updates.failure();
                    if let Some(next) = updates.pending.take() {
                        params = next;
                    }
                }
            }
            if failed {
                failure(&app, generation);
                return;
            }
        }
    });
}

pub fn reposition(app: &AppHandle) {
    let bridge = app.state::<Bridge>();
    let (connection, owner, generation) = {
        let Ok(state) = bridge.phase.lock() else {
            return;
        };
        if state.phase != Phase::Active {
            return;
        }
        let Some(lease) = state.lease.as_ref() else {
            return;
        };
        (
            lease.connection.clone(),
            lease.owner.clone(),
            state.generation,
        )
    };
    let app = app.clone();
    glib::MainContext::default().spawn_local(async move {
        if !app.state::<Bridge>().current(generation, Phase::Active) {
            return;
        }
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
    use super::{Bridge, Lifecycle, Phase, Updates};
    use glib::variant::ToVariant;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn retired_update_cannot_touch_new_generation_queue() {
        let mut state = Lifecycle {
            phase: Phase::Active,
            generation: 8,
            ..Lifecycle::default()
        };
        state.updates.generation = 8;
        assert!(state.updates.push(8_u64.to_variant()));
        assert!(state.updates_for(7).is_none());
        assert_eq!(
            state
                .updates_for(8)
                .unwrap()
                .pending
                .take()
                .unwrap()
                .get::<u64>(),
            Some(8)
        );
        state.phase = Phase::Recovering;
        assert!(state.updates_for(8).is_none());
    }

    #[test]
    fn repeated_outages_preserve_original_deadline() {
        let mut state = Lifecycle::default();
        let now = std::time::Instant::now();
        let deadline = state.outage_deadline(now);
        assert_eq!(deadline, now + Duration::from_secs(20));
        assert_eq!(
            state.outage_deadline(now + Duration::from_secs(19)),
            deadline
        );
    }

    #[test]
    fn recovering_tentative_registration_keeps_window_accessible() {
        let bridge = Bridge::default();
        {
            let mut state = bridge.phase.lock().unwrap();
            state.phase = Phase::Registering;
            state.generation = 4;
            state.deadline = Some(std::time::Instant::now() + Duration::from_secs(1));
        }
        assert!(bridge.retains_position());
        assert!(bridge.degraded());
        assert!(!bridge.current(3, Phase::Active));
        assert!(!bridge.current(4, Phase::Active));
    }

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
