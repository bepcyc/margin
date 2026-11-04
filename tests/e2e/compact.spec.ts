import { expect, test, type Page } from '@playwright/test'
import { createDemoState } from '../../src/lib/demo.ts'
import type { AppState } from '../../src/lib/model.ts'

declare global {
  interface Window { __FIT_TEST__?: number[]; __FIT_STATE__?: (state: AppState) => void }
}


async function installBridge(page: Page, state: AppState): Promise<void> {
  await page.addInitScript(state => {
    const fits: number[] = []
    window.__FIT_TEST__ = fits
    let stateHandler: ((event: { payload: unknown }) => void) | undefined
    window.__FIT_STATE__ = next => stateHandler?.({ payload: next })
    window.__TAURI__ = {
      core: {
        async invoke(command, args) {
          if (command === 'get_state') return state
          if (command === 'get_popover_placement') return { revision: 0, arrowX: null }
          if (command === 'fit_popover') fits.push(Number(args?.height))
          return undefined
        },
      },
      event: { async listen(event, handler) { if (event === 'state_changed') stateHandler = handler; return () => {} } },
    }
  }, state)
}

function bridgeState(scenario: string | null, layout: 'cards' | 'compact', count?: number): AppState {
  const state = createDemoState(scenario)
  return { ...state, accounts: state.accounts.slice(0, count), settings: { ...state.settings, overviewLayout: layout } }
}

async function lastFit(page: Page): Promise<number> {
  let previous = -1
  await expect.poll(async () => {
    const current = await page.evaluate(() => window.__FIT_TEST__?.at(-1) ?? 0)
    const stable = current > 0 && current === previous
    previous = current
    return stable
  }, { intervals: [100, 100, 200, 300] }).toBe(true)
  return previous
}

const row = (page: Page, name: string) => page.getByRole('button', { name: new RegExp(`^${name}, `) })

test('compact rows show session, weekly, and the next reset of the tighter limit', async ({ page }) => {
  await page.goto('/?demo=1&layout=compact')
  await expect(page.locator('.compact-row')).toHaveCount(3)
  await expect(page.locator('.account-card')).toHaveCount(0)
  const expected = [
    { name: 'Personal', values: ['72%', '42%'], reset: '4d 6h', low: [false, false] },
    { name: 'Work', values: ['91%', '68%'], reset: '5d 2h', low: [false, false] },
    { name: 'Studio', values: ['18%', '86%'], reset: '45m', low: [true, false] },
  ]
  for (const account of expected) {
    const button = row(page, account.name)
    await expect(button).toHaveAttribute('aria-expanded', 'false')
    await expect(button.locator('.compact-value')).toHaveText(account.values)
    await expect(button.locator('.compact-reset')).toHaveText(account.reset)
    for (const [index, low] of account.low.entries()) {
      if (low) await expect(button.locator('.compact-value').nth(index)).toHaveClass(/quota-low/)
      else await expect(button.locator('.compact-value').nth(index)).not.toHaveClass(/quota-low/)
    }
  }
  await expect(row(page, 'Studio')).toHaveAttribute('aria-label', /^Studio, Claude, hello@studio\.example, session 18 percent left, weekly 86 percent left, session resets in 45m, \d{1,2}:\d{2}/)
  await expect(row(page, 'Personal')).toHaveAttribute('aria-label', /, weekly resets in 4d 6h, \w{3} \d{1,2}:\d{2}/)
  await expect(page.locator('.compact-value').first()).toHaveCSS('font-variant-numeric', 'tabular-nums')
  await expect(page.locator('.compact-label').first()).not.toHaveAttribute('title')
  for (const [name, limiting] of [['Personal', 1], ['Studio', 0], ['Work', 1]] as const) {
    const values = row(page, name).locator('.compact-value')
    await expect(values.nth(limiting)).toHaveClass(/limiting/)
    await expect(values.nth(1 - limiting)).not.toHaveClass(/limiting/)
    const [strong, quiet] = await Promise.all([values.nth(limiting), values.nth(1 - limiting)].map(value => value.evaluate(element => getComputedStyle(element).fontWeight)))
    expect(Number(strong)).toBeGreaterThan(Number(quiet))
  }
})

test('stale, signed-out, and pinned accounts keep their markers in compact rows', async ({ page }) => {
  await page.goto('/?demo=1&state=compact')
  await expect(page.locator('.compact-row')).toHaveCount(5)
  const pinned = row(page, 'Personal')
  await expect(pinned.locator('.compact-pin')).toBeVisible()
  await expect(pinned).toHaveAttribute('aria-label', /pinned to tray$/)
  const stale = row(page, 'Side project')
  await expect(stale).toHaveClass(/account-stale/)
  await expect(stale.locator('.compact-status')).toHaveText('Last known')
  await expect(stale.locator('.compact-value')).toHaveText(['36%', '9%'])
  await expect(stale.locator('.compact-value').first()).toHaveCSS('opacity', '0.55')
  const signedOut = row(page, 'Client work for Northwind Studio')
  await expect(signedOut.locator('.compact-status')).toHaveText('Signed out')
  await expect(signedOut.locator('.compact-value')).toHaveCount(0)
  const label = signedOut.locator('.compact-label')
  await expect(label).toHaveCSS('text-overflow', 'ellipsis')
  await expect(signedOut).toHaveAttribute('aria-label', /^Client work for Northwind Studio, Claude, alex@northwind\.example, signed out$/)
  await signedOut.click()
  const card = page.getByRole('article', { name: 'Client work for Northwind Studio, Claude' })
  await expect(card.getByText('Signed out 1d ago. Run:', { exact: true })).toBeVisible()
  await expect(card.locator('.account-email')).toHaveText('alex@northwind.example')
  await expect(card.getByRole('button', { name: 'Copy command: CLAUDE_CONFIG_DIR=~/.claude-client claude', exact: true })).toBeVisible()
})

test('rows expand to the full card one at a time with mouse and keyboard', async ({ page }, info) => {
  await page.goto('/?demo=1&layout=compact')
  const personal = row(page, 'Personal')
  const work = row(page, 'Work')
  await personal.click()
  await expect(personal).toHaveAttribute('aria-expanded', 'true')
  const card = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(card).toBeVisible()
  await expect(card.getByRole('progressbar', { name: 'Session remaining', exact: true })).toBeVisible()
  await expect(card.getByRole('button', { name: 'History' })).toBeVisible()
  await expect(page.locator(`#${await personal.getAttribute('aria-controls')}`).getByRole('button', { name: 'Pin Personal to tray', exact: true })).toBeAttached()
  await page.locator('.app-shell').screenshot({ path: info.outputPath(`compact-expanded-light.png`) })
  await work.click()
  await expect(personal).toHaveAttribute('aria-expanded', 'false')
  await expect(work).toHaveAttribute('aria-expanded', 'true')
  await expect(page.locator('.account-card')).toHaveCount(1)
  await work.click()
  await expect(work).toHaveAttribute('aria-expanded', 'false')
  await expect(page.locator('.account-card')).toHaveCount(0)
  await row(page, 'Studio').focus()
  await page.keyboard.press('Enter')
  await expect(row(page, 'Studio')).toHaveAttribute('aria-expanded', 'true')
  await expect(page.getByRole('article', { name: 'Studio, Claude' })).toBeVisible()
  await page.keyboard.press('Space')
  await expect(row(page, 'Studio')).toHaveAttribute('aria-expanded', 'false')
  await expect(page.locator('.account-card')).toHaveCount(0)
})

test.describe('popover height', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' })

  test.describe('Linux panel warning', () => {
    test.use({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64)' })

    test('re-fits when a panel warning appears and disappears', async ({ page }) => {
      const state = bridgeState(null, 'compact')
      await installBridge(page, state)
      await page.goto('/')
      await expect(page.locator('.compact-row')).toHaveCount(3)
      const initial = await lastFit(page)
      await page.setViewportSize({ width: 420, height: initial })
      state.revision++
      state.bridgeNotice = 'The GNOME panel connection is unavailable. Close and restart Margin to restore the tray icon.'
      await page.evaluate(next => window.__FIT_STATE__?.(next), state)
      await expect(page.getByRole('button', { name: 'Close', exact: true })).toBeVisible()
      await expect.poll(() => lastFit(page)).toBeGreaterThan(initial)
      const withWarning = await lastFit(page)
      await page.setViewportSize({ width: 420, height: withWarning })
      await expect.poll(() => page.locator('.compact-row').last().evaluate(element => { const row = element.getBoundingClientRect(); const viewport = element.closest('.accounts-scroll')!.getBoundingClientRect(); return row.bottom <= viewport.bottom && row.top >= viewport.top })).toBe(true)
      await page.getByRole('button', { name: /^Claude 2/ }).click()
      await expect(page.locator('.compact-row')).toHaveCount(2)
      await expect.poll(() => lastFit(page)).toBeLessThan(withWarning)
      await page.setViewportSize({ width: 420, height: await lastFit(page) })
      await expect.poll(() => page.locator('.compact-row').last().evaluate(element => { const row = element.getBoundingClientRect(); const viewport = element.closest('.accounts-scroll')!.getBoundingClientRect(); return row.bottom <= viewport.bottom && row.top >= viewport.top })).toBe(true)
      state.revision++
      state.bridgeNotice = null
      const filteredWithWarning = await lastFit(page)
      await page.evaluate(next => window.__FIT_STATE__?.(next), state)
      await expect.poll(() => lastFit(page)).toBeLessThan(filteredWithWarning)
      await page.getByRole('button', { name: /^All 3/ }).click()
      await expect.poll(() => lastFit(page)).toBe(initial)
    })
  })

  test('compact fits a shorter window and expand re-measures', async ({ page }, info) => {
    const heights: Record<string, number> = {}
    for (const [key, scenario, count] of [['1 account', 'single', 1], ['5 accounts', 'compact', 5]] as const) {
      for (const layout of ['cards', 'compact'] as const) {
        const tab = await page.context().newPage()
        await installBridge(tab, bridgeState(scenario, layout, count))
        await tab.goto('/')
        await expect(tab.locator(layout === 'compact' ? '.compact-row' : '.account-card')).toHaveCount(count)
        heights[`${layout}, ${key}`] = await lastFit(tab)
        await tab.close()
      }
    }
    await info.attach('fit-heights', { body: JSON.stringify(heights, null, 2), contentType: 'application/json' })
    console.log(`[${info.project.name}] fit heights ${JSON.stringify(heights)}`)
    expect(heights['compact, 1 account']).toBeLessThan(heights['cards, 1 account']!)
    expect(heights['compact, 5 accounts']).toBeLessThan(heights['cards, 5 accounts']!)
    expect(heights['cards, 5 accounts']).toBe(660)
    expect(heights['compact, 1 account']).toBeLessThan(360)

    await installBridge(page, bridgeState(null, 'compact'))
    await page.goto('/')
    await expect(page.locator('.compact-row')).toHaveCount(3)
    const collapsed = await lastFit(page)
    await row(page, 'Work').click()
    await expect.poll(() => lastFit(page)).toBeGreaterThan(collapsed)
    await row(page, 'Work').click()
    await expect.poll(() => lastFit(page)).toBe(collapsed)
    await page.getByRole('button', { name: 'Open settings', exact: true }).click()
    expect(await lastFit(page)).toBe(660)
  })
})

test('preview popover is shorter in compact and screenshots in both themes', async ({ page }, info) => {
  const heights: Record<string, number> = {}
  for (const theme of ['light', 'dark'] as const) {
    for (const [key, query] of [['1-account', 'state=single&layout=compact'], ['5-accounts', 'state=compact']] as const) {
      await page.goto(`/?demo=1&theme=${theme}&${query}`)
      await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
      await expect(page.locator('.compact-row')).toHaveCount(key === '1-account' ? 1 : 5)
      const shell = page.locator('.app-shell')
      await expect(shell).toHaveClass(/fit-content/)
      const box = await page.locator('.popover').boundingBox()
      heights[key] = Math.round(box?.height ?? 0)
      const bottom = await page.locator('.app-footer').evaluate(element => element.getBoundingClientRect().bottom)
      expect(bottom).toBeLessThanOrEqual(page.viewportSize()?.height ?? 0)
      await shell.screenshot({ path: info.outputPath(`compact-${key}-${theme}.png`) })
    }
  }
  await page.goto('/?demo=1&state=single')
  const cardsSingle = Math.round((await page.locator('.popover').boundingBox())?.height ?? 0)
  console.log(`[${info.project.name}] preview popover heights ${JSON.stringify({ ...heights, 'cards-1-account': cardsSingle })}`)
  expect(heights['1-account']).toBeLessThan(cardsSingle)
  expect(heights['1-account']).toBeLessThan(heights['5-accounts']!)
})
