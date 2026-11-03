<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch, watchEffect } from 'vue'
import { z } from 'zod'
import { TooltipProvider } from 'reka-ui'
import AppTooltip from './components/ui/AppTooltip.vue'
import AccountCard from './components/AccountCard.vue'
import AccountRow from './components/AccountRow.vue'
import AppIcon from './components/AppIcon.vue'
import SettingsPanel from './components/SettingsPanel.vue'
import { accountIsStale, orderAccounts, relativeAge } from './lib/format'
import { useMargin } from './lib/margin'
import { useUpdater } from './lib/updater'
import { createDemoUpdateState } from './lib/demo'
import { liveRestoreOffer, popoverShownSchema, type Account, type AccountId, type Provider, type Settings } from './lib/model'

const { state, loading, connectionError, pending, message, now, demo, desktop, refresh, discover, restoreIgnored, chooseFolder, addAccount, removeAccount, renameAccount, saveSettings, resetStore, hideWindow, previewMakeActive, makeActive, restoreSwitchBackup, dismissSwitchRestore } = useMargin()
const updater = useUpdater({ demo, desktop, initial: demo ? createDemoUpdateState(new URLSearchParams(location.search).get('state')) : undefined })
const popoverPlacementSchema = z.object({ revision: z.number().int().nonnegative(), arrowX: z.number().finite().nullable() })
const popoverPlacement = ref<z.infer<typeof popoverPlacementSchema>>({ revision: 0, arrowX: null })
const viewportWidth = ref(window.innerWidth)
const isWindows = navigator.userAgent.includes('Windows')
const isLinux = navigator.userAgent.includes('Linux')
const usesControlKey = isWindows || isLinux
const nativeArrowX = computed(() => {
  const x = popoverPlacement.value.arrowX
  if (!desktop || demo || usesControlKey || x === null) return null
  return x >= 26 && x <= viewportWidth.value - 26 ? x : null
})
const nativeArrowStyle = computed(() => nativeArrowX.value === null ? undefined : { left: `${nativeArrowX.value - 6}px` })
const page = ref<'overview' | 'settings'>('overview')
const settingsTab = ref<'general' | 'accounts'>('general')
const filter = ref<'all' | Provider>('all')
const media = window.matchMedia('(prefers-color-scheme: dark)')
const systemDark = ref(media.matches)
const dark = computed(() => state.value.settings.theme === 'dark' || (state.value.settings.theme === 'system' && systemDark.value))
const busy = computed(() => pending.value !== null)
const refreshing = computed(() => (!connectionError.value && state.value.refreshing) || pending.value === 'refresh_accounts' || pending.value === 'discover_accounts' || pending.value === 'restore_ignored_accounts')
const accountOrder = ref<AccountId[]>([])
const orderedAccounts = computed(() => {
  const position = new Map(accountOrder.value.map((id, index) => [id, index]))
  return state.value.accounts
    .map((account, index) => ({ account, rank: position.get(account.id) ?? accountOrder.value.length + index }))
    .sort((a, b) => a.rank - b.rank)
    .map(entry => entry.account)
})
const visibleAccounts = computed(() => orderedAccounts.value.filter(account => filter.value === 'all' || account.provider === filter.value))
const needsAttention = (account: Account): boolean => accountIsStale(account, state.value.settings, now.value, Boolean(connectionError.value))
const staleCount = computed(() => state.value.accounts.filter(needsAttention).length)
const counts = computed(() => ({ all: state.value.accounts.length, claude: state.value.accounts.filter(account => account.provider === 'claude').length, codex: state.value.accounts.filter(account => account.provider === 'codex').length }))
const showFilters = computed(() => counts.value.claude > 0 && counts.value.codex > 0)
const filterOptions: { id: 'all' | Provider; label: string }[] = [{ id: 'all', label: 'All' }, { id: 'claude', label: 'Claude' }, { id: 'codex', label: 'Codex' }]
const compact = computed(() => state.value.settings.overviewLayout === 'compact')
const expandedId = ref<AccountId | null>(null)
const isActive = (account: Account): boolean => state.value.switching.activeAccountIds.includes(account.id)
const isPinned = (account: Account): boolean => state.value.settings.trayMode === 'pinned' && state.value.settings.pinnedAccountId === account.id
const fitsContent = computed(() => page.value === 'overview' && !loading.value && state.value.accounts.length > 0)
const shell = ref<HTMLElement | null>(null)
const popover = ref<HTMLElement | null>(null)
const scroller = ref<HTMLElement | null>(null)
const accountList = ref<HTMLElement | null>(null)
const fullHeight = 660
const fitHeight = ref(0)
let fitFrame = 0
let fitObserver: ResizeObserver | undefined
let stopSettingsListener: (() => void) | undefined
let stopPlacementListener: (() => void) | undefined
let stopShownListener: (() => void) | undefined
let destroyed = false
const latestFetch = computed(() => {
  const timestamps = state.value.accounts.flatMap(account => account.snapshot ? [account.snapshot.fetchedAt] : [])
  return timestamps.sort((a, b) => Date.parse(b) - Date.parse(a)).at(0)
})
const showsAttention = computed(() => !refreshing.value && !connectionError.value && staleCount.value > 0)
const status = computed(() => {
  if (refreshing.value) return 'Checking usage'
  if (connectionError.value) return 'Connection interrupted'
  if (staleCount.value) return `${staleCount.value} ${staleCount.value === 1 ? 'account needs' : 'accounts need'} attention`
  if (latestFetch.value) return `Updated ${relativeAge(latestFetch.value, now.value)}`
  return state.value.accounts.length ? 'Waiting for usage' : 'No accounts connected'
})

function sortAccounts(): void {
  accountOrder.value = orderAccounts(state.value.accounts, state.value.settings, now.value, Boolean(connectionError.value)).map(account => account.id)
}

watch(() => state.value.accounts.map(account => account.id).join('\n'), sortAccounts, { immediate: true })
watch(() => state.value.settings.accountOrder, sortAccounts)
watch(loading, value => { if (!value) sortAccounts() })

watch(counts, value => {
  if (filter.value !== 'all' && (!value.claude || !value.codex)) filter.value = 'all'
})

watchEffect(() => {
  document.documentElement.dataset.theme = dark.value ? 'dark' : 'light'
  document.documentElement.dataset.platform = isWindows ? 'windows' : isLinux ? 'linux' : 'mac'
  document.documentElement.style.colorScheme = dark.value ? 'dark' : 'light'
})

function toggleExpanded(id: AccountId): void {
  expandedId.value = expandedId.value === id ? null : id
}

function measureFit(): void {
  fitFrame = 0
  let height = fullHeight
  if (fitsContent.value && shell.value && popover.value && scroller.value) {
    const style = getComputedStyle(shell.value)
    height = Math.ceil(parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + popover.value.offsetHeight - scroller.value.clientHeight + scroller.value.scrollHeight)
  }
  height = Math.min(height, fullHeight)
  if (height === fitHeight.value) return
  fitHeight.value = height
  if (desktop && !demo) window.__TAURI__?.core.invoke('fit_popover', { height }).catch(() => {})
}

function scheduleFit(): void {
  if (!fitFrame) fitFrame = requestAnimationFrame(measureFit)
}

function observeFit(): void {
  fitObserver?.disconnect()
  for (const element of [popover.value, accountList.value]) if (element) fitObserver?.observe(element)
  scheduleFit()
}

watch([fitsContent, compact, () => state.value.accounts.length], () => { void nextTick(observeFit) })
watch(compact, () => { expandedId.value = null })

let settingsRequestedAt = Number.NEGATIVE_INFINITY
let pickingFolder = false
let folderPickEndedAt = Number.NEGATIVE_INFINITY

async function pickFolder(provider: Provider): Promise<string | null> {
  pickingFolder = true
  try {
    return await chooseFolder(provider)
  } finally {
    pickingFolder = false
    folderPickEndedAt = Date.now()
  }
}

function onPopoverShown(payload: unknown): void {
  const result = popoverShownSchema.safeParse(payload)
  if (!result.success) return
  const { hiddenForMs } = result.data
  if (hiddenForMs !== null && hiddenForMs < 60_000) return
  if (pickingFolder || (hiddenForMs !== null && Date.now() - folderPickEndedAt < hiddenForMs + 1000)) return
  now.value = Date.now()
  sortAccounts()
  expandedId.value = null
  if (performance.now() - settingsRequestedAt > 1000) page.value = 'overview'
  void nextTick(() => scroller.value?.scrollTo({ top: 0 }))
}

async function showAttention(): Promise<void> {
  const target = orderedAccounts.value.find(needsAttention)
  if (!target) return
  if (filter.value !== 'all' && filter.value !== target.provider) filter.value = 'all'
  if (compact.value) expandedId.value = target.id
  await nextTick()
  const id = CSS.escape(target.id)
  const element = accountList.value?.querySelector<HTMLElement>(compact.value ? `.compact-row[aria-controls="account-${id}"]` : `.account-card[data-account-id="${id}"]`)
  if (!element) return
  const smooth = !window.matchMedia('(prefers-reduced-motion: reduce)').matches
  ;(element.closest('li') ?? element).scrollIntoView({ block: 'nearest', behavior: smooth ? 'smooth' : 'auto' })
  element.focus({ preventScroll: true })
}

function moveRowFocus(event: KeyboardEvent): void {
  const target = event.target
  if (!(target instanceof HTMLElement) || !target.classList.contains('compact-row')) return
  const rows = [...(accountList.value?.querySelectorAll<HTMLElement>('.compact-row') ?? [])]
  const index = rows.indexOf(target)
  const next = { ArrowDown: index + 1, ArrowUp: index - 1, Home: 0, End: rows.length - 1 }[event.key]
  const row = next === undefined ? undefined : rows[next]
  if (!row) return
  event.preventDefault()
  row.focus()
}

async function setLayout(layout: Settings['overviewLayout']): Promise<void> {
  if (state.value.settings.overviewLayout !== layout) await saveSettings({ ...state.value.settings, overviewLayout: layout })
}

function openSettings(tab: 'general' | 'accounts' = 'general'): void {
  settingsTab.value = tab
  page.value = 'settings'
}

async function togglePin(account: Account): Promise<void> {
  const pinned = state.value.settings.trayMode === 'pinned' && state.value.settings.pinnedAccountId === account.id
  await saveSettings({ ...state.value.settings, trayMode: pinned ? 'lowest' : 'pinned', pinnedAccountId: pinned ? null : account.id })
}

function syncSystemTheme(event: MediaQueryListEvent): void { systemDark.value = event.matches }
function syncViewportWidth(): void { viewportWidth.value = window.innerWidth }

function acceptPopoverPlacement(payload: unknown): void {
  if (destroyed) return
  const result = popoverPlacementSchema.safeParse(payload)
  if (!result.success) {
    popoverPlacement.value = { ...popoverPlacement.value, arrowX: null }
    return
  }
  if (result.data.revision >= popoverPlacement.value.revision) popoverPlacement.value = result.data
}

async function connectPopoverPlacement(): Promise<void> {
  const tauri = window.__TAURI__
  if (!tauri) return
  try {
    const unlisten = await tauri.event.listen('popover_placement_changed', event => acceptPopoverPlacement(event.payload))
    if (destroyed) { unlisten(); return }
    stopPlacementListener = unlisten
    acceptPopoverPlacement(await tauri.core.invoke('get_popover_placement'))
  } catch {
    if (!destroyed) popoverPlacement.value = { ...popoverPlacement.value, arrowX: null }
  }
}

function onKeydown(event: KeyboardEvent): void {
  if (event.defaultPrevented) return
  if (document.querySelector('[role="alertdialog"]')) {
    if ((event.metaKey || event.ctrlKey) && [',', 'r', '1', '2'].includes(event.key.toLowerCase())) event.preventDefault()
    return
  }
  if (event.key === 'Escape') {
    if (page.value === 'settings') page.value = 'overview'
    else void hideWindow()
  }
  if ((event.metaKey || event.ctrlKey) && event.key === ',') { event.preventDefault(); openSettings() }
  if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'r') { event.preventDefault(); void refresh() }
  if ((event.metaKey || event.ctrlKey) && !event.altKey && !event.shiftKey && (event.key === '1' || event.key === '2')) {
    event.preventDefault()
    void setLayout(event.key === '1' ? 'cards' : 'compact')
  }
}

onMounted(async () => {
  media.addEventListener('change', syncSystemTheme)
  window.addEventListener('keydown', onKeydown)
  window.addEventListener('resize', syncViewportWidth)
  if (typeof ResizeObserver === 'function') fitObserver = new ResizeObserver(scheduleFit)
  observeFit()
  if (!desktop || demo) return
  void connectPopoverPlacement()
  try {
    const unlisten = await window.__TAURI__?.event.listen('open_settings', () => { settingsRequestedAt = performance.now(); openSettings() })
    if (destroyed) unlisten?.()
    else stopSettingsListener = unlisten
  } catch { }
  try {
    const unlisten = await window.__TAURI__?.event.listen('popover_shown', event => onPopoverShown(event.payload))
    if (destroyed) unlisten?.()
    else stopShownListener = unlisten
  } catch { }
})
onUnmounted(() => {
  destroyed = true
  fitObserver?.disconnect()
  cancelAnimationFrame(fitFrame)
  stopSettingsListener?.()
  stopPlacementListener?.()
  stopShownListener?.()
  media.removeEventListener('change', syncSystemTheme)
  window.removeEventListener('keydown', onKeydown)
  window.removeEventListener('resize', syncViewportWidth)
})
</script>

<template>
  <TooltipProvider :delay-duration="350">
  <div class="preview-stage" :class="{ 'desktop-stage': desktop && !demo }">
    <div v-if="!desktop && !demo" class="desktop-gate">
      <span class="gate-logo"><AppIcon name="margin" :size="36"/></span>
      <p class="eyebrow">CLAUDE + CODEX</p><h1>Track your usage.</h1>
      <p class="gate-description">See account limits and reset times in the menu bar or system tray.</p>
      <a class="button button-primary" href="https://github.com/Abulkosim/margin/releases" target="_blank" rel="noopener noreferrer">Download Margin<AppIcon name="external" :size="14"/></a>
      <a class="preview-link" href="?demo=1">Preview <span>with sample data</span><AppIcon name="chevron" :size="12"/></a>
      <p class="gate-footnote">Reads CLI sign-ins. Changes them only when you ask.</p>
    </div>

    <div v-else ref="shell" class="app-shell" :class="{ 'is-preview': demo, 'fit-content': fitsContent }" :data-fit-height="fitHeight">
      <div v-if="demo || nativeArrowX !== null" class="popover-arrow" :style="demo ? undefined : nativeArrowStyle" aria-hidden="true"/>
      <div ref="popover" class="popover">
        <div v-if="state.recovery" class="notice-banner recovery-banner" role="alert"><AppIcon name="alert" :size="13"/><div><p>Settings file couldn’t be read. Reset keeps a backup next to it.</p><p v-if="state.notice" class="recovery-detail">{{ state.notice }}</p><button class="button button-small" :disabled="busy" @click="resetStore">Back up and reset settings</button></div></div>
        <div v-else-if="state.notice" class="notice-banner" role="alert"><AppIcon name="alert" :size="13"/><span>{{ state.notice }}</span></div>
        <template v-if="page === 'overview'">
          <h1 class="sr-only">Usage remaining</h1>
          <header class="app-header"><div class="wordmark"><AppIcon name="margin" :size="18"/><span>margin</span></div><div v-if="showFilters" class="account-filters" role="group" aria-label="Filter by provider"><button v-for="option in filterOptions" :key="option.id" :class="{ active: filter === option.id }" :aria-pressed="filter === option.id" @click="filter = option.id">{{ option.label }}<span>{{ counts[option.id] }}</span></button></div><div class="header-actions"><AppTooltip :text="refreshing ? 'Checking usage' : `Refresh usage (${usesControlKey ? 'Ctrl+R' : '⌘R'})`"><button class="icon-button refresh-button" :class="{ pulsing: refreshing }" aria-label="Refresh usage" :disabled="busy || refreshing" @click="refresh"><AppIcon name="refresh" :size="16"/></button></AppTooltip><AppTooltip :text="`Settings (${usesControlKey ? 'Ctrl+,' : '⌘,'})`"><button class="icon-button settings-button" :aria-label="updater.available.value ? 'Settings, update available' : 'Open settings'" @click="openSettings()"><AppIcon name="settings" :size="18"/><span v-if="updater.available.value" class="update-dot" aria-hidden="true"/></button></AppTooltip></div></header>

          <main ref="scroller" class="accounts-scroll" :class="{ 'empty-scroll': !state.accounts.length }" aria-label="Account usage">
            <div v-if="loading" class="initial-loading" role="status" aria-label="Loading accounts"><div v-for="index in 2" :key="index" class="skeleton-card"><div class="skeleton skeleton-heading"/><div class="skeleton skeleton-label"/><div class="skeleton skeleton-bar"/><div class="skeleton skeleton-label"/><div class="skeleton skeleton-bar"/></div><p>Reading your accounts…</p></div>
            <div v-else-if="connectionError && !state.accounts.length" class="empty-state"><span class="load-error-symbol"><AppIcon name="alert" :size="28"/></span><h2>Couldn’t load your accounts.</h2><p>{{ connectionError }}</p><button class="button button-primary" :disabled="busy" @click="refresh"><AppIcon name="refresh" :size="14"/>Try again</button></div>
            <div v-else-if="!state.accounts.length" class="empty-state"><div class="empty-illustration" aria-hidden="true"><div class="empty-orbit orbit-one"/><div class="empty-orbit orbit-two"/><span class="empty-provider empty-claude"><AppIcon name="claude" :size="24"/></span><span class="empty-provider empty-codex"><AppIcon name="codex" :size="23"/></span><span class="empty-center"><AppIcon name="margin" :size="27"/></span></div><h2>Add your accounts</h2><p>Connect Claude or Codex accounts already signed in on this device.</p><button class="button button-primary" :disabled="busy" @click="discover"><AppIcon name="refresh" :size="14"/>Find accounts</button><button class="text-button" @click="openSettings('accounts')">Add a config folder<AppIcon name="plus" :size="13"/></button><span class="empty-privacy">Reads CLI sign-ins. Changes them only when you ask.</span></div>
            <div v-else ref="accountList" class="account-list">
              <template v-if="compact && visibleAccounts.length">
                <div class="compact-columns" aria-hidden="true"><span>Session</span><span>Weekly</span><span>Resets</span></div>
                <ul class="compact-list" @keydown="moveRowFocus">
                  <li v-for="account in visibleAccounts" :key="account.id" class="compact-item" :class="{ expanded: expandedId === account.id }">
                    <AccountRow :account="account" :settings="state.settings" :now="now" :pinned="isPinned(account)" :expanded="expandedId === account.id" :connection-error="connectionError" :controls="`account-${account.id}`" @toggle="toggleExpanded(account.id)"/>
                    <div v-if="expandedId === account.id" :id="`account-${account.id}`" class="compact-detail"><AccountCard :account="account" :demo="demo" :settings="state.settings" :now="now" :pinned="isPinned(account)" :active="isActive(account)" :busy="busy" :connection-error="connectionError" @pin="togglePin(account)" @refresh="refresh"/></div>
                  </li>
                </ul>
              </template>
              <template v-else-if="!compact"><AccountCard v-for="account in visibleAccounts" :key="account.id" :account="account" :demo="demo" :settings="state.settings" :now="now" :pinned="isPinned(account)" :active="isActive(account)" :busy="busy" :connection-error="connectionError" @pin="togglePin(account)" @refresh="refresh"/></template>
              <div v-if="!visibleAccounts.length" class="filter-empty"><AppIcon :name="filter === 'claude' ? 'claude' : 'codex'" :size="25"/><p>No {{ filter === 'claude' ? 'Claude' : 'Codex' }} accounts yet.</p><button class="text-button" @click="openSettings('accounts')">Add an account<AppIcon name="plus" :size="13"/></button></div>
            </div>
          </main>
          <footer class="app-footer"><span class="sync-status" :class="{ warning: staleCount || connectionError, pulsing: refreshing }" role="status"><span class="status-dot"/><button v-if="showsAttention" class="text-button attention-button" @click="showAttention">{{ status }}</button><template v-else>{{ status }}</template></span><button class="text-button footer-add" @click="openSettings('accounts')"><AppIcon name="plus" :size="13"/>Add account</button></footer>
        </template>

        <SettingsPanel v-else :update-state="updater.state.value" :update-busy="updater.busy.value" :demo="demo" @check-update="updater.check" @install-update="updater.install" :accounts="state.accounts" :recovered="state.recovered" :hidden-count="state.ignoredSources.length" :can-choose-folder="desktop || demo" :choose-folder="pickFolder" :settings="state.settings" :busy="busy" :initial-tab="settingsTab" :save="saveSettings" :add="addAccount" :remove="removeAccount" :rename="renameAccount" :active-ids="state.switching.activeAccountIds" :restore-offer="liveRestoreOffer(state.switching.restore, now)" :preview-make-active="previewMakeActive" :make-active="makeActive" :restore-backup="restoreSwitchBackup" :dismiss-restore="dismissSwitchRestore" @back="page = 'overview'" @discover="discover" @show-hidden="restoreIgnored"/>
        <div v-if="demo" class="preview-ribbon"><span class="preview-dot"/>Interface preview<span>Sample data</span></div>
        <Transition name="toast"><div v-if="message" class="toast-message" :class="{ 'toast-error': message.kind === 'error' }" :role="message.kind === 'error' ? 'alert' : 'status'"><AppIcon :name="message.kind === 'error' ? 'alert' : 'check'" :size="15"/><span>{{ message.text }}</span><button v-if="message.action" class="toast-action" :disabled="busy" @click="message.action.run()">{{ message.action.label }}</button><button class="icon-button" aria-label="Dismiss message" @click="message = null"><AppIcon name="close" :size="13"/></button></div></Transition>
      </div>
    </div>
  </div>
  </TooltipProvider>
</template>

<style scoped>
.settings-button { position: relative; }
.attention-button { padding: 0; font-size: inherit; color: inherit; text-decoration: underline; text-decoration-color: color-mix(in srgb,currentColor 30%,transparent); text-underline-offset: 3px; }
.attention-button:hover { color: inherit; text-decoration-color: currentColor; }
.update-dot { position: absolute; top: 6px; right: 6px; width: 6px; height: 6px; border-radius: 50%; background: var(--accent); box-shadow: 0 0 0 1.5px var(--surface); }
</style>
