import { expect, test, type Locator, type Page } from '@playwright/test'
import { createDemoState } from '../../src/lib/demo.ts'
import { accountSchema, type AppState } from '../../src/lib/model.ts'

declare global {
  interface Window { __CONTROL_TEST__?: { rejectPending: () => void; saved: unknown[]; commands: { command: string; args?: Record<string, unknown> }[] } }
}

async function installDesktopBridge(page: Page, options: { state: AppState; failure: 'none' | 'save' | 'remove' }): Promise<void> {
  await page.addInitScript(({ state, failure }) => {
    let settings: unknown = state.settings
    let accounts = state.accounts
    let revision = state.revision
    let ignoredSources = state.ignoredSources
    const listeners = new Map<string, (event: { payload: unknown }) => void>()
    let rejectPending: (() => void) | undefined
    const saved: unknown[] = []
    const commands: { command: string; args?: Record<string, unknown> }[] = []
    window.__CONTROL_TEST__ = { rejectPending: () => { rejectPending?.(); rejectPending = undefined }, saved, commands }
    function fail(message: string): Promise<never> {
      return new Promise((_resolve, reject) => { rejectPending = () => reject(new Error(message)) })
    }
    window.__TAURI__ = {
      core: {
        async invoke(command, args) {
          if (!['get_state', 'get_popover_placement', 'fit_popover'].includes(command)) commands.push({ command, args })
          if (command === 'get_state') return { ...state, revision, settings, accounts, ignoredSources }
          if (command === 'plugin:dialog|open') return '/Users/alex/.claude-work'
          if (command === 'restore_ignored_accounts') {
            ignoredSources = []
            revision++
          }
          if (command === 'get_popover_placement') return { revision: 0, arrowX: null }
          if (command === 'save_settings') {
            if (failure === 'save') return fail('Could not save settings. Try again.')
            settings = args?.settings
            saved.push(settings)
            revision++
          }
          if (command === 'remove_account') {
            if (failure === 'remove') return fail('Could not remove account. Try again.')
            accounts = accounts.filter(account => account.id !== args?.id)
            revision++
          }
          if (command === 'hide_window') document.getElementById('app')?.setAttribute('hidden', '')
          return undefined
        },
      },
      event: {
        async listen(event, handler) {
          listeners.set(event, handler)
          return () => { listeners.delete(event) }
        },
      },
    }
  }, options)
}

async function openSettings(page: Page): Promise<void> {
  await page.getByRole('button', { name: 'Open settings', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
}

test('panel recovery notice survives a refresh', async ({ page }) => {
  const state = createDemoState(null)
  state.bridgeNotice = 'The GNOME panel connection is unavailable. Close and restart Margin to restore the tray icon.'
  await installDesktopBridge(page, { state, failure: 'none' })
  await page.goto('/')
  const notice = page.getByRole('alert').filter({ hasText: state.bridgeNotice })
  await expect(notice).toBeVisible()
  await page.getByRole('button', { name: 'Refresh usage' }).click()
  await expect(notice).toBeVisible()
  const close = notice.getByRole('button', { name: 'Close', exact: true })
  await close.focus()
  await page.keyboard.press('Enter')
  await expect.poll(() => page.evaluate(() => window.__CONTROL_TEST__?.commands.some(entry => entry.command === 'hide_window'))).toBe(true)
})

test('panel recovery close works by click', async ({ page }) => {
  const state = createDemoState(null)
  state.bridgeNotice = 'The GNOME panel connection is unavailable. Close and restart Margin to restore the tray icon.'
  await installDesktopBridge(page, { state, failure: 'none' })
  await page.goto('/')
  await page.getByRole('alert').getByRole('button', { name: 'Close', exact: true }).click()
  await expect.poll(() => page.evaluate(() => window.__CONTROL_TEST__?.commands.some(entry => entry.command === 'hide_window'))).toBe(true)
})

async function expectWithinViewport(page: Page, locator: Locator): Promise<void> {
  await expect(locator).toBeVisible()
  const bounds = await locator.evaluate(element => {
    const rect = element.getBoundingClientRect()
    return { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom }
  })
  const viewport = await page.evaluate(() => ({ width: innerWidth, height: innerHeight }))
  expect(bounds.left).toBeGreaterThanOrEqual(0)
  expect(bounds.top).toBeGreaterThanOrEqual(0)
  expect(bounds.right).toBeLessThanOrEqual(viewport.width)
  expect(bounds.bottom).toBeLessThanOrEqual(viewport.height)
}

test('selects support Arrow, Home, End, and typeahead selection', async ({ page }) => {
  await page.goto('/?demo=1')
  await openSettings(page)
  const interval = page.getByRole('combobox', { name: 'Refresh while open', exact: true })
  await interval.focus()
  await page.keyboard.press('ArrowDown')
  await expect(page.getByRole('listbox')).toBeVisible()
  await expect(page.getByRole('option', { name: '1 minute', exact: true })).toBeFocused()
  await page.keyboard.press('ArrowDown')
  await expect(page.getByRole('option', { name: '2 minutes', exact: true })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(interval).toContainText('2 minutes')
  await expect(interval).toBeFocused()

  await page.keyboard.press('Enter')
  await expect(page.getByRole('option', { name: '2 minutes', exact: true })).toBeFocused()
  await page.keyboard.press('End')
  await expect(page.getByRole('option', { name: '5 minutes', exact: true })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(interval).toContainText('5 minutes')
  await page.keyboard.press('Space')
  await expect(page.getByRole('option', { name: '5 minutes', exact: true })).toBeFocused()
  await page.keyboard.press('Home')
  await expect(page.getByRole('option', { name: '30 seconds', exact: true })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(interval).toContainText('30 seconds')

  const tray = page.getByRole('combobox', { name: 'Tray account', exact: true })
  await tray.focus()
  await page.keyboard.press('Space')
  await expect(page.getByRole('listbox')).toBeVisible()
  await page.keyboard.type('Stu')
  await expect(page.getByRole('option', { name: 'Studio', exact: true })).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(tray).toContainText('Studio')
  await expect(tray).toBeFocused()
})

test('tray metric defaults to session and weekly and saves the choice', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const metric = page.getByRole('combobox', { name: 'Tray shows', exact: true })
  await expect(metric).toContainText('Session + weekly')
  await metric.click()
  await expect(page.getByRole('option')).toHaveText(['Session + weekly', 'Session', 'Weekly', 'Any limit'])
  await page.getByRole('option', { name: 'Weekly', exact: true }).click()
  await expect(metric).toContainText('Weekly')
  await expect(metric).toBeFocused()
  const saved = await page.evaluate(() => window.__CONTROL_TEST__?.saved ?? [])
  expect(saved).toHaveLength(1)
  expect(saved[0]).toMatchObject({ trayMetric: 'weekly', trayMode: 'lowest' })
})

test.describe('macOS tray percentage', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' })

  test('shows the toggle off by default and saves it', async ({ page }) => {
    await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
    await page.goto('/')
    await openSettings(page)
    const toggle = page.getByRole('switch', { name: 'Show percentage next to icon', exact: true })
    await expect(toggle).toHaveAttribute('aria-checked', 'false')
    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-checked', 'true')
    const saved = await page.evaluate(() => window.__CONTROL_TEST__?.saved ?? [])
    expect(saved).toHaveLength(1)
    expect(saved[0]).toMatchObject({ trayShowPercent: true, trayMetric: 'core' })
  })
})

test.describe('Windows tray percentage', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' })

  test('hides the macOS-only toggle', async ({ page }) => {
    await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
    await page.goto('/')
    await openSettings(page)
    await expect(page.getByRole('combobox', { name: 'Tray shows', exact: true })).toBeVisible()
    await expect(page.getByText('Show percentage next to icon')).toHaveCount(0)
  })
})

test('Escape closes a select before leaving Settings and returns trigger focus', async ({ page }) => {
  await page.goto('/?demo=1')
  await openSettings(page)
  const trigger = page.getByRole('combobox', { name: 'Refresh while open', exact: true })
  await trigger.click()
  await expect(page.getByRole('listbox')).toBeVisible()
  await page.keyboard.press('End')
  await page.keyboard.press('Escape')
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await expect(trigger).toContainText('1 minute')
  await expect(trigger).toBeFocused()
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
  await page.keyboard.press('Escape')
  await expect(page.getByRole('article').first()).toBeVisible()
})

for (const theme of ['light', 'dark']) {
  test(`${theme} custom menus remain inside a narrow desktop window`, async ({ page }, info) => {
    const state = createDemoState(null)
    state.settings.theme = theme === 'light' ? 'light' : 'dark'
    await installDesktopBridge(page, { state, failure: 'none' })
    await page.setViewportSize({ width: 360, height: 480 })
    await page.goto('/')
    await openSettings(page)
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
    for (const label of ['Tray account', 'Tray shows', 'Refresh while open', 'Refresh in background']) {
      const trigger = page.getByRole('combobox', { name: label, exact: true })
      await trigger.click()
      const menu = page.getByRole('listbox')
      await expectWithinViewport(page, menu)
      await expect(menu.getByRole('option').first()).toBeVisible()
      await info.attach(`${theme}-${label}`, { body: await page.screenshot(), contentType: 'image/png' })
      await page.keyboard.press('Escape')
      await expect(menu).toHaveCount(0)
      await expect(trigger).toBeFocused()
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
  })
}

test('a hundred long account names stay scrollable and keyboard reachable', async ({ page }, info) => {
  const state = createDemoState(null)
  const lastLabel = 'Zeta workspace with a very long account label'
  state.accounts = Array.from({ length: 100 }, (_, index) => accountSchema.parse({
    id: `control-account-${index}`,
    provider: index % 2 === 0 ? 'claude' : 'codex',
    label: index === 99 ? lastLabel : `Workspace ${String(index + 1).padStart(3, '0')} · Research and production teams`,
    color: '#b8d1bf',
    source: { kind: 'configDir', path: `/example/cli/account-${index}` },
    identity: { email: null, plan: null },
    snapshot: null,
    error: null,
    refreshing: false,
  }))
  await installDesktopBridge(page, { state, failure: 'none' })
  await page.setViewportSize({ width: 360, height: 480 })
  await page.goto('/')
  await openSettings(page)
  const trigger = page.getByRole('combobox', { name: 'Tray account', exact: true })
  await trigger.focus()
  await page.keyboard.press('Space')
  const menu = page.getByRole('listbox')
  await expect(menu.getByRole('option')).toHaveCount(101)
  await expectWithinViewport(page, menu)
  await expect(page.getByRole('option', { name: 'Lowest remaining', exact: true })).toBeFocused()
  await page.keyboard.press('End')
  const lastOption = page.getByRole('option', { name: lastLabel, exact: true })
  await expect(lastOption).toBeFocused()
  await expect(lastOption).toBeInViewport({ ratio: 1 })
  await expectWithinViewport(page, lastOption)
  await info.attach('long-account-menu', { body: await page.screenshot(), contentType: 'image/png' })
  await page.keyboard.press('Home')
  await expect(page.getByRole('option', { name: 'Lowest remaining', exact: true })).toBeFocused()
  await page.keyboard.type('Zeta')
  await expect(lastOption).toBeFocused()
  await page.keyboard.press('Enter')
  await expect(trigger).toContainText(lastLabel)
  await expect(trigger).toBeFocused()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true)
})

test('settings tabs and reminder switch work entirely from the keyboard', async ({ page }) => {
  await page.goto('/?demo=1')
  await openSettings(page)
  const general = page.getByRole('tab', { name: 'General', exact: true })
  const accounts = page.getByRole('tab', { name: /^Accounts/ })
  await general.focus()
  await page.keyboard.press('ArrowRight')
  await expect(accounts).toBeFocused()
  await expect(accounts).toHaveAttribute('aria-selected', 'true')
  await expect(page.getByRole('heading', { name: 'Your accounts', exact: true })).toBeVisible()
  await page.keyboard.press('ArrowLeft')
  await expect(general).toBeFocused()
  await expect(general).toHaveAttribute('aria-selected', 'true')
  const reminders = page.getByRole('switch', { name: 'Usage alerts', exact: true })
  await reminders.focus()
  await page.keyboard.press('Space')
  await expect(reminders).toHaveAttribute('aria-checked', 'true')
  await expect(page.getByRole('button', { name: 'Notify at 20% left', exact: true })).toBeEnabled()
  await expect(reminders).toBeFocused()
  await page.keyboard.press('Space')
  await expect(reminders).toHaveAttribute('aria-checked', 'false')
  await expect(page.getByRole('button', { name: 'Notify at 20% left', exact: true })).toBeDisabled()
})

test('remove confirmation traps focus and Escape restores its trigger', async ({ page }) => {
  await page.goto('/?demo=1')
  await openSettings(page)
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  const trigger = page.getByRole('button', { name: 'Remove Personal', exact: true })
  await trigger.click()
  const dialog = page.getByRole('alertdialog')
  const keep = dialog.getByRole('button', { name: 'Keep account', exact: true })
  const remove = dialog.getByRole('button', { name: 'Remove account', exact: true })
  await expect(keep).toBeFocused()
  await page.keyboard.press('Shift+Tab')
  await expect(remove).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(keep).toBeFocused()
  await page.keyboard.press('Tab')
  await expect(remove).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await expect(trigger).toBeFocused()
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
  await trigger.click()
  await keep.click()
  await expect(dialog).toHaveCount(0)
  await expect(trigger).toBeFocused()
})

test('failed removal keeps the confirmation and the account', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'remove' })
  await page.goto('/')
  await openSettings(page)
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  const trigger = page.getByRole('button', { name: 'Remove Personal', exact: true })
  await trigger.click()
  const dialog = page.getByRole('alertdialog')
  await dialog.getByRole('button', { name: 'Remove account', exact: true }).click()
  await expect(dialog.getByRole('button', { name: 'Remove account', exact: true })).toBeDisabled()
  await expect(dialog.getByRole('button', { name: 'Keep account', exact: true })).toBeDisabled()
  await page.keyboard.press('Escape')
  await expect(dialog).toBeVisible()
  await page.evaluate(() => window.__CONTROL_TEST__?.rejectPending())
  await expect(page.getByText('Could not remove account. Try again.', { exact: true })).toBeVisible()
  await expect(dialog).toBeVisible()
  await expect(dialog.getByRole('alert')).toBeVisible()
  await expect(dialog.getByRole('button', { name: 'Remove account', exact: true })).toBeEnabled()
  await expect(dialog.getByRole('button', { name: 'Remove account', exact: true })).toBeFocused()
  await dialog.getByRole('button', { name: 'Keep account', exact: true }).click()
  await expect(trigger).toBeFocused()
  await expect(page.getByRole('heading', { name: 'Personal', exact: true })).toBeVisible()
})

test('hidden removed accounts can be shown again', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState('hidden'), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  const note = page.getByText('2 removed accounts hidden from Find accounts', { exact: false })
  await expect(note).toBeVisible()
  await page.getByRole('button', { name: 'Show them', exact: true }).click()
  await expect(note).toHaveCount(0)
  const commands = await page.evaluate(() => window.__CONTROL_TEST__?.commands.map(entry => entry.command) ?? [])
  expect(commands.filter(command => !['get_update_status', 'get_account_history'].includes(command))).toEqual(['restore_ignored_accounts'])
})

test('no hidden note without removed accounts', async ({ page }) => {
  await page.goto('/?demo=1')
  await openSettings(page)
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  await expect(page.getByRole('heading', { name: 'Your accounts', exact: true })).toBeVisible()
  await expect(page.getByText(/hidden from Find accounts/)).toHaveCount(0)
  await page.getByRole('button', { name: 'Remove Personal', exact: true }).click()
  await page.getByRole('alertdialog').getByRole('button', { name: 'Remove account', exact: true }).click()
  await expect(page.getByText('1 removed account hidden from Find accounts', { exact: false })).toBeVisible()
})

test('folder picker fills the path and typing still works', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  await page.getByRole('button', { name: 'Add account', exact: true }).click()
  const input = page.getByLabel('Config folder', { exact: true })
  await input.fill('~/.claude-typed')
  await expect(input).toHaveValue('~/.claude-typed')
  await page.getByRole('button', { name: 'Choose folder…', exact: true }).click()
  await expect(input).toHaveValue('/Users/alex/.claude-work')
  await expect(input).toBeFocused()
  await page.getByRole('button', { name: 'Connect account', exact: true }).click()
  const commands = await page.evaluate(() => window.__CONTROL_TEST__?.commands ?? [])
  expect(commands.filter(entry => !['get_update_status', 'get_account_history'].includes(entry.command))).toEqual([
    { command: 'plugin:dialog|open', args: { options: { directory: true, multiple: false, title: 'Choose the Claude config folder' } } },
    { command: 'add_account', args: { provider: 'claude', path: '/Users/alex/.claude-work' } },
  ])
})

for (const [platform, userAgent, claudeHint] of [
  ['macOS', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36', 'Sign in there first: CLAUDE_CONFIG_DIR=~/.claude-work claude'],
  ['Windows', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36', 'The folder containing .credentials.json. Sign in with the CLI first.'],
] as const) {
  test.describe(`add-account hint on ${platform}`, () => {
    test.use({ userAgent })

    test('names the sign-in step per provider and follows the typed folder', async ({ page }) => {
      await page.goto('/?demo=1')
      await openSettings(page)
      await page.getByRole('tab', { name: /^Accounts/ }).click()
      await page.getByRole('button', { name: 'Add account', exact: true }).click()
      const hint = page.locator('#path-help')
      const input = page.getByLabel('Config folder', { exact: true })
      await expect(hint).toHaveText(claudeHint)
      await input.fill('~/.claude-team')
      await expect(hint).toHaveText(platform === 'macOS' ? 'Sign in there first: CLAUDE_CONFIG_DIR=~/.claude-team claude' : claudeHint)
      await input.fill('')
      await page.getByRole('button', { name: 'Codex', exact: true }).click()
      await expect(hint).toHaveText('The folder containing auth.json. Sign in first: CODEX_HOME=~/.codex-work codex login')
      await input.fill('~/.codex-team')
      await expect(hint).toHaveText('The folder containing auth.json. Sign in first: CODEX_HOME=~/.codex-team codex login')
    })
  })
}

test('folder picker fills a sample path in the preview', async ({ page }) => {
  await page.goto('/?demo=1')
  await openSettings(page)
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  await page.getByRole('button', { name: 'Add account', exact: true }).click()
  await page.getByRole('button', { name: 'Codex', exact: true }).click()
  await page.getByRole('button', { name: 'Choose folder…', exact: true }).click()
  await expect(page.getByLabel('Config folder', { exact: true })).toHaveValue('~/.codex-work')
})

test('failed saves keep the previously saved select and switch values', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'save' })
  await page.goto('/')
  await openSettings(page)
  const interval = page.getByRole('combobox', { name: 'Refresh while open', exact: true })
  await interval.click()
  await page.getByRole('option', { name: '2 minutes', exact: true }).click()
  await expect(interval).toBeDisabled()
  await page.evaluate(() => window.__CONTROL_TEST__?.rejectPending())
  await expect(page.getByText('Could not save settings. Try again.', { exact: true })).toBeVisible()
  await expect(page.getByRole('listbox')).toHaveCount(0)
  await expect(interval).toContainText('1 minute')
  await expect(interval).toBeFocused()
  const reminders = page.getByRole('switch', { name: 'Usage alerts', exact: true })
  await reminders.focus()
  await page.keyboard.press('Space')
  await expect(reminders).toBeDisabled()
  await page.evaluate(() => window.__CONTROL_TEST__?.rejectPending())
  await expect(reminders).toBeEnabled()
  await expect(reminders).toHaveAttribute('aria-checked', 'false')
  await expect(reminders).toBeFocused()
})

test('a delayed save does not steal focus moved to another control', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'save' })
  await page.goto('/')
  await openSettings(page)
  const interval = page.getByRole('combobox', { name: 'Refresh while open', exact: true })
  await interval.click()
  await page.getByRole('option', { name: '2 minutes', exact: true }).click()
  await expect(interval).toBeDisabled()
  const general = page.getByRole('tab', { name: 'General', exact: true })
  await general.focus()
  await page.evaluate(() => window.__CONTROL_TEST__?.rejectPending())
  await expect(interval).toBeEnabled()
  await expect(interval).toContainText('1 minute')
  await expect(general).toBeFocused()
})

test('tooltips support focus and Escape without native title bubbles', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
  await page.goto('/')
  await expect(page.locator('[title]')).toHaveCount(0)
  const settings = page.getByRole('button', { name: 'Open settings', exact: true })
  await settings.hover()
  await expect(page.getByRole('tooltip')).toContainText('Settings')
  await page.mouse.move(1, 1)
  await expect(page.getByRole('tooltip')).toHaveCount(0)
  await settings.focus()
  await expect(page.getByRole('tooltip')).toContainText('Settings')
  await page.keyboard.press('Escape')
  await expect(page.getByRole('tooltip')).toHaveCount(0)
  await expect(settings).toBeFocused()
  await expect(page.getByRole('article').first()).toBeVisible()
  await page.keyboard.press('Enter')
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  await expect(page.locator('[title]')).toHaveCount(0)
})

function notificationState(settings: Partial<AppState['settings']> = {}): AppState {
  const state = createDemoState(null)
  return { ...state, settings: { ...state.settings, notifications: true, ...settings } }
}

async function savedSettings(page: Page): Promise<AppState['settings'][]> {
  return page.evaluate(() => (window.__CONTROL_TEST__?.saved ?? []) as AppState['settings'][])
}

test('threshold chips toggle and save sorted thresholds', async ({ page }) => {
  await installDesktopBridge(page, { state: notificationState(), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const chip = (used: number) => page.getByRole('button', { name: `Notify at ${100 - used}% left`, exact: true })
  await expect(page.locator('.threshold-chip')).toHaveText(['50%', '20%', '10%', '5%', '0%'])
  await expect(page.locator('.threshold-options')).toHaveText(/^Notify at.*0%left$/)
  await expect(chip(80)).toHaveAttribute('aria-pressed', 'true')
  await expect(chip(50)).toHaveAttribute('aria-pressed', 'false')
  await chip(50).click()
  await expect(chip(50)).toHaveAttribute('aria-pressed', 'true')
  await chip(95).click()
  await expect(chip(95)).toHaveAttribute('aria-pressed', 'false')
  await chip(90).click()
  await expect(chip(90)).toHaveAttribute('aria-pressed', 'true')
  expect((await savedSettings(page)).map(settings => settings.thresholds)).toEqual([[50, 80, 95, 100], [50, 80, 100], [50, 80, 90, 100]])
})

test('the last selected threshold cannot be cleared', async ({ page }) => {
  await installDesktopBridge(page, { state: notificationState({ thresholds: [95] }), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const only = page.getByRole('button', { name: 'Notify at 5% left', exact: true })
  await expect(only).toHaveAttribute('aria-pressed', 'true')
  await expect(only).toBeDisabled()
  await page.getByRole('button', { name: 'Notify at 20% left', exact: true }).click()
  await expect(only).toBeEnabled()
  expect((await savedSettings(page)).map(settings => settings.thresholds)).toEqual([[80, 95]])
})

test('reset alerts save and follow the usage alerts switch', async ({ page }) => {
  await installDesktopBridge(page, { state: notificationState(), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const reset = page.getByRole('switch', { name: 'Notify when limits reset', exact: true })
  await expect(reset).toHaveAttribute('aria-checked', 'false')
  await reset.click()
  await expect(reset).toHaveAttribute('aria-checked', 'true')
  await page.getByRole('switch', { name: 'Usage alerts', exact: true }).click()
  await expect(reset).toBeDisabled()
  const saved = await savedSettings(page)
  expect(saved).toHaveLength(2)
  expect(saved[0]).toMatchObject({ notifyOnReset: true, notifications: true })
  expect(saved[1]).toMatchObject({ notifyOnReset: true, notifications: false })
  await expect(page.getByText(/Notifications are off in/)).toHaveCount(0)
})

test('run-out and not-updating alerts start on, save, and follow usage alerts', async ({ page }) => {
  await installDesktopBridge(page, { state: notificationState(), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const pace = page.getByRole('switch', { name: 'Run-out warnings', exact: true })
  const stale = page.getByRole('switch', { name: 'Notify when usage stops updating', exact: true })
  await expect(pace).toHaveAttribute('aria-checked', 'true')
  await expect(stale).toHaveAttribute('aria-checked', 'true')
  await expect(page.getByText('When a limit is on pace to run out before it resets.', { exact: true })).toBeVisible()
  await expect(page.getByText('When an account can’t be read for 30 minutes.', { exact: true })).toBeVisible()
  await pace.click()
  await expect(pace).toHaveAttribute('aria-checked', 'false')
  await stale.click()
  await expect(stale).toHaveAttribute('aria-checked', 'false')
  const saved = await savedSettings(page)
  expect(saved.map(settings => [settings.paceAlerts, settings.staleAlerts])).toEqual([[false, true], [false, false]])
  await page.getByRole('switch', { name: 'Usage alerts', exact: true }).click()
  await expect(pace).toBeDisabled()
  await expect(stale).toBeDisabled()
})

test('fast usage alerts start off, save a threshold, and keep it on reopen', async ({ page }) => {
  await installDesktopBridge(page, { state: notificationState(), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const burn = page.getByRole('switch', { name: 'Fast usage alerts', exact: true })
  const chip = (value: number) => page.getByRole('button', { name: `${value}% within an hour`, exact: true })
  await expect(burn).toHaveAttribute('aria-checked', 'false')
  await expect(page.getByText('When a limit uses 20% or more within an hour.', { exact: true })).toBeVisible()
  await expect(chip(20)).toHaveCount(0)
  await burn.click()
  await expect(burn).toHaveAttribute('aria-checked', 'true')
  await expect(page.locator('.burn-options .threshold-chip')).toHaveText(['10%', '20%', '30%'])
  await expect(page.getByRole('group', { name: 'Fast usage threshold', exact: true })).toBeVisible()
  await expect(chip(20)).toHaveAttribute('aria-pressed', 'true')
  await chip(20).click()
  await chip(10).click()
  await expect(chip(10)).toHaveAttribute('aria-pressed', 'true')
  await expect(chip(20)).toHaveAttribute('aria-pressed', 'false')
  await expect(page.getByText('When a limit uses 10% or more within an hour.', { exact: true })).toBeVisible()
  const saved = await savedSettings(page)
  expect(saved.map(settings => [settings.burnAlerts, settings.burnThreshold])).toEqual([[true, 20], [true, 10]])
  await page.getByRole('button', { name: 'Back to overview', exact: true }).click()
  await openSettings(page)
  await expect(burn).toHaveAttribute('aria-checked', 'true')
  await expect(chip(10)).toHaveAttribute('aria-pressed', 'true')
  await page.getByRole('switch', { name: 'Usage alerts', exact: true }).click()
  await expect(burn).toBeDisabled()
  await expect(chip(30)).toBeDisabled()
  expect((await savedSettings(page)).at(-1)).toMatchObject({ notifications: false, burnAlerts: true, burnThreshold: 10 })
})

for (const theme of ['light', 'dark'] as const) {
  test(`${theme} notification settings with fast usage alerts`, async ({ page }, info) => {
    await page.goto(`/?demo=1&theme=${theme}`)
    await openSettings(page)
    await page.getByRole('switch', { name: 'Usage alerts', exact: true }).click()
    await page.getByRole('switch', { name: 'Fast usage alerts', exact: true }).click()
    const section = page.locator('.settings-section').filter({ has: page.getByRole('heading', { name: 'Notifications', exact: true }) })
    await expect(section.locator('.burn-options')).toBeVisible()
    await section.scrollIntoViewIfNeeded()
    await expectWithinViewport(page, section.locator('.burn-options'))
    await page.screenshot({ path: info.outputPath(`settings-notifications-${theme}.png`) })
  })
}

test('denied notifications explain where to turn them on without a dead button', async ({ page }) => {
  await installDesktopBridge(page, { state: notificationState({ notificationPermission: 'denied' }), failure: 'none' })
  await page.goto('/')
  await openSettings(page)
  const note = page.getByRole('status').filter({ hasText: /Notifications are off in (System|Windows) Settings\./ })
  await expect(note).toBeVisible()
  await expect(note.getByRole('button')).toHaveCount(0)
})

test.describe('desktop update settings', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' })

  test('daily update checks are on by default and the switch saves the choice', async ({ page }) => {
    await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
    await page.goto('/')
    await openSettings(page)
    const daily = page.getByRole('region', { name: 'Updates', exact: true }).getByRole('switch', { name: 'Check for updates daily', exact: true })
    await expect(daily).toHaveAttribute('aria-checked', 'true')
    await daily.click()
    await expect(daily).toHaveAttribute('aria-checked', 'false')
    const saved = await savedSettings(page)
    expect(saved).toHaveLength(1)
    expect(saved[0]).toMatchObject({ autoCheckUpdates: false })
  })
})

test('overview layout defaults to cards and Compact saves the choice', async ({ page }) => {
  await installDesktopBridge(page, { state: createDemoState(null), failure: 'none' })
  await page.goto('/')
  await expect(page.locator('.account-card')).toHaveCount(3)
  await openSettings(page)
  const layout = page.getByRole('combobox', { name: 'Overview', exact: true })
  await expect(layout).toContainText('Cards')
  await layout.click()
  await expect(page.getByRole('option')).toHaveText(['Cards', 'Compact'])
  await page.getByRole('option', { name: 'Compact', exact: true }).click()
  await expect(layout).toContainText('Compact')
  const saved = await savedSettings(page)
  expect(saved).toHaveLength(1)
  expect(saved[0]).toMatchObject({ overviewLayout: 'compact', theme: 'system' })
  await page.getByRole('button', { name: 'Back to overview', exact: true }).click()
  await expect(page.locator('.compact-row')).toHaveCount(3)
  await expect(page.locator('.account-card')).toHaveCount(0)
})
