import { onMounted, onUnmounted, ref } from 'vue'
import { createDemoState, demoActivate, demoPreview } from './demo.ts'
import { accountSchema, activatedSchema, appStateSchema, defaultSettings, makeActivePreviewSchema, type AccountId, type AppState, type MakeActivePreview, type Provider, type Settings } from './model.ts'

declare global {
  interface Window {
    __TAURI__?: {
      core: { invoke: (command: string, args?: Record<string, unknown>) => Promise<unknown> }
      event: { listen: (event: string, handler: (event: { payload: unknown }) => void) => Promise<() => void> }
    }
  }
}

type Command = 'refresh_accounts' | 'discover_accounts' | 'restore_ignored_accounts' | 'add_account' | 'remove_account' | 'rename_account' | 'save_settings' | 'reset_store' | 'preview_make_active' | 'make_active' | 'restore_switch_backup' | 'dismiss_switch_restore'
export type ToastAction = { label: string; run: () => void }

export function useMargin() {
  const query = new URLSearchParams(window.location.search)
  const demo = query.get('demo') === '1'
  const desktop = typeof window.__TAURI__?.core.invoke === 'function'
  const state = ref<AppState>(demo ? createDemoState(query.get('state')) : { revision: 0, accounts: [], settings: { ...defaultSettings }, refreshing: false, notice: null, bridgeNotice: null, recovery: false, recovered: [], ignoredSources: [], switching: { activeAccountIds: [], restore: null } })
  const demoRunning = query.get('running') === '1'
  const loading = ref(desktop && !demo)
  const connectionError = ref<string | null>(null)
  const pending = ref<Command | null>(null)
  const message = ref<{ kind: 'error' | 'success'; text: string; action?: ToastAction } | null>(null)
  const now = ref(Date.now())
  let stopListening: (() => void) | undefined
  let connectingListener: Promise<void> | null = null
  let destroyed = false
  let toastTimer: ReturnType<typeof setTimeout> | undefined
  let clock: number | undefined
  const tick = (): void => { now.value = Date.now() }
  function syncClock(): void {
    clearInterval(clock)
    clock = undefined
    if (globalThis.document?.visibilityState === 'hidden') return
    tick()
    clock = window.setInterval(tick, 1000)
  }
  syncClock()
  globalThis.document?.addEventListener('visibilitychange', syncClock)

  const theme = query.get('theme')
  if (demo && (theme === 'light' || theme === 'dark')) state.value.settings.theme = theme
  const layout = query.get('layout')
  if (demo && (layout === 'cards' || layout === 'compact')) state.value.settings.overviewLayout = layout
  const order = query.get('order')
  if (demo && (order === 'urgency' || order === 'added')) state.value.settings.accountOrder = order

  function notify(text: string, kind: 'error' | 'success' = 'success', action?: ToastAction): void {
    clearTimeout(toastTimer)
    message.value = { kind, text, action }
    toastTimer = setTimeout(() => { message.value = null }, kind === 'error' || action ? 9000 : 3500)
  }

  function acceptState(payload: unknown): boolean {
    const result = appStateSchema.safeParse(payload)
    if (result.success) {
      if (result.data.revision < state.value.revision) return false
      state.value = result.data
      connectionError.value = null
      return true
    } else {
      connectionError.value = 'Margin received an unreadable response. Refresh to try again.'
      throw new Error(connectionError.value)
    }
  }

  async function listenToState(): Promise<void> {
    if (stopListening || destroyed) return
    if (connectingListener) return connectingListener
    connectingListener = (async () => {
      const tauri = window.__TAURI__
      if (!tauri) throw new Error('Desktop connection unavailable')
      const unlisten = await tauri.event.listen('state_changed', event => {
        try { acceptState(event.payload) }
        catch { notify(connectionError.value ?? 'Could not read the latest account state.', 'error') }
      })
      if (destroyed) unlisten()
      else stopListening = unlisten
    })()
    try { await connectingListener }
    finally { connectingListener = null }
  }

  async function reload(): Promise<void> {
    let payload: unknown
    try {
      await listenToState()
      const tauri = window.__TAURI__
      if (!tauri) throw new Error('Desktop connection unavailable')
      payload = await tauri.core.invoke('get_state')
    } catch {
      connectionError.value = 'Could not read your accounts. Refresh to reconnect.'
      throw new Error(connectionError.value)
    }
    const accepted = acceptState(payload)
    if (!accepted && connectionError.value) throw new Error(connectionError.value)
  }

  async function perform(command: Command, args?: Record<string, unknown>, preview?: () => void): Promise<boolean> {
    if (pending.value) return false
    pending.value = command
    try {
      if (demo) {
        preview?.()
        state.value.revision++
      }
      else {
        const tauri = window.__TAURI__
        if (!tauri) throw new Error('Open the desktop app to manage your accounts.')
        await tauri.core.invoke(command, args)
        await reload()
      }
      return true
    } catch (error: unknown) {
      notify(error instanceof Error ? error.message : typeof error === 'string' ? error : 'That did not work. Please try again.', 'error')
      return false
    } finally {
      pending.value = null
    }
  }

  async function refresh(): Promise<void> {
    await perform('refresh_accounts', undefined, () => {
      state.value.refreshing = false
      state.value.accounts = state.value.accounts.map(account => ({
        ...account, refreshing: false,
        snapshot: account.snapshot ? { ...account.snapshot, fetchedAt: new Date().toISOString() } : null,
      }))
      notify('Preview refreshed. Values are sample data.')
    })
  }

  async function discover(): Promise<void> {
    if (await perform('discover_accounts', undefined, () => {
      if (!state.value.accounts.length) state.value.accounts = createDemoState(null).accounts
    })) notify(demo ? 'Sample accounts found.' : 'Account discovery complete.')
  }

  async function restoreIgnored(): Promise<void> {
    if (await perform('restore_ignored_accounts', undefined, () => { state.value.ignoredSources = [] }))
      notify(demo ? 'Hidden accounts shown in preview.' : 'Account discovery complete.')
  }

  async function chooseFolder(provider: Provider): Promise<string | null> {
    if (demo) return provider === 'claude' ? '~/.claude-work' : '~/.codex-work'
    try {
      const tauri = window.__TAURI__
      if (!tauri) throw new Error('Desktop connection unavailable')
      const selected = await tauri.core.invoke('plugin:dialog|open', {
        options: { directory: true, multiple: false, title: `Choose the ${provider === 'claude' ? 'Claude' : 'Codex'} config folder` },
      })
      return typeof selected === 'string' && selected ? selected : null
    } catch {
      notify('Could not open the folder picker. Type the folder path instead.', 'error')
      return null
    }
  }

  async function addAccount(provider: Provider, path: string): Promise<boolean> {
    const added = await perform('add_account', { provider, path }, () => {
      state.value.accounts.push(accountSchema.parse({
        id: `demo-${crypto.randomUUID()}`, provider,
        label: path.split(/[\\/]/).filter(Boolean).at(-1) ?? 'New account', color: '#b8d1bf',
        source: { kind: 'configDir', path }, identity: { email: null, plan: null },
        snapshot: null, refreshing: false,
        error: { kind: 'preview', message: 'This is a preview. The desktop app reads usage for accounts added here.', retryAfterSeconds: null },
      }))
    })
    if (added) notify(demo ? 'Account added to preview.' : 'Account added.')
    return added
  }

  async function removeAccount(id: AccountId): Promise<boolean> {
    const removed = await perform('remove_account', { id }, () => {
      const account = state.value.accounts.find(account => account.id === id)
      if (account) state.value.ignoredSources = [...state.value.ignoredSources, `${account.provider}:${account.source.kind === 'configDir' ? account.source.path : 'keychain'}`]
      state.value.accounts = state.value.accounts.filter(account => account.id !== id)
      if (state.value.settings.pinnedAccountId === id) state.value.settings = {
        ...state.value.settings,
        pinnedAccountId: state.value.accounts.at(0)?.id ?? null,
      }
    })
    if (removed) notify('Account removed from Margin.')
    return removed
  }

  async function renameAccount(id: AccountId, label: string): Promise<boolean> {
    return perform('rename_account', { id, label }, () => {
      state.value.accounts = state.value.accounts.map(account => account.id === id ? { ...account, label } : account)
    })
  }

  async function saveSettings(settings: Settings): Promise<boolean> {
    return perform('save_settings', { settings }, () => { state.value.settings = settings })
  }

  async function resetStore(): Promise<boolean> {
    const reset = await perform('reset_store', undefined, () => {
      state.value = { ...createDemoState(null), revision: state.value.revision, settings: { ...defaultSettings, theme: state.value.settings.theme } }
    })
    if (reset) notify('Settings reset. The old file was kept as a backup.')
    return reset
  }

  async function call(command: Command, args: Record<string, unknown>, demoResult: () => unknown): Promise<unknown> {
    if (pending.value) return undefined
    pending.value = command
    try {
      if (demo) {
        const result = demoResult()
        if (typeof result === 'string') throw new Error(result)
        return result
      }
      const tauri = window.__TAURI__
      if (!tauri) throw new Error('Open the desktop app to manage your accounts.')
      return await tauri.core.invoke(command, args)
    } catch (error: unknown) {
      notify(error instanceof Error ? error.message : typeof error === 'string' ? error : 'That did not work. Please try again.', 'error')
      return undefined
    } finally {
      pending.value = null
    }
  }

  async function previewMakeActive(id: AccountId): Promise<MakeActivePreview | null> {
    const result = await call('preview_make_active', { id }, () => demoPreview(state.value, id, demoRunning))
    if (result === undefined) return null
    const preview = makeActivePreviewSchema.safeParse(result)
    if (!preview.success) notify('Margin received an unreadable response. Try again.', 'error')
    return preview.success ? preview.data : null
  }

  async function makeActive(preview: MakeActivePreview, undo = true): Promise<boolean> {
    const result = await call('make_active', { id: preview.accountId, fingerprint: preview.fingerprint }, () => {
      const next = demoActivate(state.value, preview.accountId)
      return { state: { ...next.state, revision: state.value.revision + 1 }, target: preview.target, previousAccountId: next.previousAccountId }
    })
    if (result === undefined) return false
    const activated = activatedSchema.safeParse(result)
    if (!activated.success) {
      notify('Margin received an unreadable response. Refresh to try again.', 'error')
      return false
    }
    if (activated.data.state.revision >= state.value.revision) state.value = activated.data.state
    const previous = activated.data.previousAccountId
    notify(`${activated.data.target} is active.${preview.provider === 'codex' ? ' Reopen the Codex app to use it.' : ''}`, 'success', undo && previous ? { label: 'Undo', run: () => { void switchBack(previous) } } : undefined)
    return true
  }

  async function switchBack(id: AccountId): Promise<void> {
    message.value = null
    const preview = await previewMakeActive(id)
    if (preview) await makeActive(preview, false)
  }

  async function restoreSwitchBackup(id: string): Promise<boolean> {
    const restored = await perform('restore_switch_backup', { id }, () => { state.value.switching = { ...state.value.switching, restore: null } })
    if (restored) notify('Backup restored.')
    return restored
  }

  async function dismissSwitchRestore(id: string): Promise<boolean> {
    return perform('dismiss_switch_restore', { id }, () => { state.value.switching = { ...state.value.switching, restore: null } })
  }

  async function hideWindow(): Promise<void> {
    if (!desktop || demo) return
    try {
      const tauri = window.__TAURI__
      if (!tauri) throw new Error('Desktop connection unavailable')
      await tauri.core.invoke('hide_window')
    } catch {
      notify('Could not hide the window. Try again.', 'error')
    }
  }

  onMounted(async () => {
    if (!desktop || demo) return
    try {
      await reload()
    } catch {
      connectionError.value ??= 'Could not connect to Margin. Close and reopen the app to try again.'
      notify(connectionError.value, 'error')
    } finally {
      loading.value = false
    }
  })

  onUnmounted(() => {
    destroyed = true
    stopListening?.()
    clearInterval(clock)
    globalThis.document?.removeEventListener('visibilitychange', syncClock)
    clearTimeout(toastTimer)
  })

  return { state, loading, connectionError, pending, message, now, demo, desktop, refresh, discover, restoreIgnored, chooseFolder, addAccount, removeAccount, renameAccount, saveSettings, resetStore, hideWindow, previewMakeActive, makeActive, restoreSwitchBackup, dismissSwitchRestore }
}
