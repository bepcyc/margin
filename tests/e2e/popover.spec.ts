import { expect, test } from '@playwright/test'

test('browser entry never pretends to read real accounts', async ({ page }) => {
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Track your usage.' })).toBeVisible()
  await expect(page.getByRole('article')).toHaveCount(0)
  await page.getByRole('link', { name: /Preview with sample data/ }).click()
  await expect(page.getByText('Sample data', { exact: true })).toBeVisible()
})

test('loads the bundled interface font', async ({ page }) => {
  await page.goto('/?demo=1')
  const fonts = await page.evaluate(async () => (await document.fonts.load('650 15px Inter')).map(font => ({ family: font.family, status: font.status })))
  expect(fonts).toEqual([{ family: 'Inter', status: 'loaded' }])
})

test.describe('Linux presentation', () => {
  test.use({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36' })

  test('uses Linux platform, control shortcuts and no popover arrow', async ({ page }) => {
    await page.goto('/?demo=1')
    await expect(page.locator('html')).toHaveAttribute('data-platform', 'linux')
    await expect(page.locator('.popover-arrow')).toBeHidden()
    await page.getByRole('button', { name: 'Open settings' }).hover()
    await expect(page.getByRole('tooltip')).toHaveText('Settings (Ctrl+,)')
  })
})

for (const theme of ['dark', 'light']) {
  test(`${theme} overview fits a tray-sized window`, async ({ page }, info) => {
    const errors: string[] = []
    page.on('pageerror', error => errors.push(error.message))
    await page.goto(`/?demo=1&theme=${theme}`)
    await expect(page.locator('html')).toHaveAttribute('data-theme', theme)
    await expect(page.getByRole('article')).toHaveCount(3)
    await expect(page.getByRole('heading', { name: 'Usage remaining', level: 1 })).toBeAttached()
    await expect(page.getByRole('article').first()).toBeVisible()
    await expect(page.getByRole('group', { name: 'Filter by provider' })).toBeVisible()
    await expect(page.getByText('Sample data', { exact: true })).toBeVisible()
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
    await info.attach(`overview-${theme}`, { body: await page.screenshot(), contentType: 'image/png' })
    expect(errors).toEqual([])
  })
}

test('provider filters and account details are interactive', async ({ page }) => {
  await page.goto('/?demo=1')
  await page.getByRole('button', { name: /^Codex/ }).click()
  await expect(page.getByRole('article')).toHaveCount(1)
  await expect(page.getByRole('heading', { name: 'Work', exact: true })).toBeVisible()
  await page.getByRole('button', { name: /^All/ }).click()
  const work = page.getByRole('article', { name: 'Work, Codex' })
  await expect(work.getByText('Spark · session', { exact: true })).toHaveCount(0)
  await work.getByRole('button', { name: '1 more window' }).click()
  await expect(work.getByText('Spark · session', { exact: true })).toBeVisible()
  await expect(work.getByRole('button', { name: 'Show less' })).toHaveAttribute('aria-expanded', 'true')
  const personal = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(personal.getByRole('button', { name: /more window/ })).toHaveCount(0)
  await personal.getByRole('button', { name: 'Pin Personal to tray' }).click()
  await expect(personal.getByRole('button', { name: 'Unpin Personal from tray' })).toHaveAttribute('aria-pressed', 'true')
})

test('model limits show as chips under the weekly row', async ({ page }) => {
  await page.goto('/?demo=1&order=added')
  const personal = page.getByRole('article', { name: 'Personal, Claude' })
  const chips = personal.getByRole('list', { name: 'Model limits' }).getByRole('listitem')
  await expect(chips).toHaveCount(3)
  await expect(personal.locator('.model-chip-name')).toHaveText(['Fable', 'Opus', 'Sonnet'])
  await expect(personal.locator('.model-chip-value')).toHaveText(['34%', '12%', '66%'])
  await expect(personal.locator('.model-chip').nth(1)).toHaveClass(/quota-low/)
  await expect(personal.locator('.quota-window')).toHaveCount(2)
  const weekly = await personal.locator('.quota-window').nth(1).boundingBox()
  const row = await personal.locator('.model-chips').boundingBox()
  expect(weekly && row && row.y >= weekly.y + weekly.height).toBe(true)
  const fable = personal.getByRole('progressbar', { name: 'Fable weekly remaining', exact: true })
  await expect(fable).toHaveAttribute('aria-valuenow', '34')
  await expect(fable).toHaveAttribute('aria-valuetext', /^34 percent remaining, resets in 4d \d+h$/)
  await page.keyboard.press('Tab')
  await fable.focus()
  await expect(page.getByRole('tooltip')).toHaveText(/^Fable weekly · resets in 4d \d+h$/)
  const studio = page.getByRole('article', { name: 'Studio, Claude' })
  await expect(studio.locator('.model-chip-name')).toHaveText(['Sonnet'])
  const work = page.getByRole('article', { name: 'Work, Codex' })
  await expect(work.locator('.model-chip-name')).toHaveText(['Spark'])
  await expect(work.locator('.model-chip-value')).toHaveText(['81%'])
  await expect(work.getByText('Spark · session', { exact: true })).toHaveCount(0)
})

test('balances sit under the details toggle, credits as a unit and money only for Claude', async ({ page }) => {
  await page.goto('/?demo=1&order=added')
  const work = page.getByRole('article', { name: 'Work, Codex' })
  await expect(work.locator('.balance-lines')).toHaveCount(0)
  await work.getByRole('button', { name: '1 more window' }).click()
  const workLines = work.locator('.balance-line')
  await expect(workLines).toHaveText(['Credits 1,250 left', /^Monthly credit limit 320 of 500 used · resets \w{3} \d{1,2}$/])
  await expect(work.locator('.balance-lines')).not.toContainText('$')
  const studio = page.getByRole('article', { name: 'Studio, Claude' })
  await expect(studio.getByRole('button', { name: 'Extra usage' })).toHaveAttribute('aria-expanded', 'false')
  await studio.getByRole('button', { name: 'Extra usage' }).click()
  await expect(studio.locator('.balance-line')).toHaveText(['Extra usage $4.28 of $20 this month'])
  const personal = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(personal.locator('.details-toggle:not(.history-toggle)')).toHaveCount(0)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})

for (const theme of ['light', 'dark']) {
  test(`${theme} balance screenshots`, async ({ page }, info) => {
    await page.goto(`/?demo=1&theme=${theme}&order=added`)
    const work = page.getByRole('article', { name: 'Work, Codex' })
    await work.getByRole('button', { name: '1 more window' }).click()
    await expect(work.locator('.balance-line')).toHaveCount(2)
    await work.screenshot({ path: info.outputPath(`balance-codex-${theme}.png`) })
    const studio = page.getByRole('article', { name: 'Studio, Claude' })
    await studio.getByRole('button', { name: 'Extra usage' }).click()
    await expect(studio.locator('.balance-line')).toHaveCount(1)
    await studio.screenshot({ path: info.outputPath(`balance-claude-${theme}.png`) })
  })
}

test('limit resets are listed read-only before history', async ({ page }) => {
  await page.goto('/?demo=1')
  const studio = page.getByRole('article', { name: 'Studio, Claude' })
  const line = studio.locator('.reset-grants')
  await expect(line).toHaveText('1 reset available · expires in 3d')
  const chips = await studio.locator('.quota-list').boundingBox()
  const position = await line.boundingBox()
  const history = await studio.getByRole('button', { name: 'History' }).boundingBox()
  expect(chips && position && history && position.y >= chips.y + chips.height && history.y >= position.y + position.height).toBe(true)
  await expect(page.locator('.reset-grants')).toHaveCount(1)
  for (const card of await page.getByRole('article').all()) {
    await expect(card.getByRole('button', { name: /reset/i })).toHaveCount(0)
    await expect(card.getByRole('link', { name: /reset/i })).toHaveCount(0)
  }
})

test('model chips wrap at 360px without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 640 })
  await page.goto('/?demo=1')
  const personal = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(personal.locator('.model-chip')).toHaveCount(3)
  await personal.locator('.model-chips').evaluate(list => {
    for (const item of [...list.children]) list.append(item.cloneNode(true))
  })
  const rows = await personal.locator('.model-chip').evaluateAll(chips => new Set(chips.map(chip => Math.round(chip.getBoundingClientRect().top))).size)
  expect(rows).toBeGreaterThan(1)
  const list = await personal.locator('.model-chips').boundingBox()
  for (const box of await personal.locator('.model-chip').evaluateAll(chips => chips.map(chip => chip.getBoundingClientRect().toJSON() as DOMRect))) {
    expect(list && box.right <= list.x + list.width + 0.5).toBe(true)
  }
  expect(await personal.locator('.model-chips').evaluate(list => list.scrollWidth <= list.clientWidth)).toBe(true)
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})

for (const theme of ['light', 'dark']) {
  test(`${theme} model chips screenshot`, async ({ page }, info) => {
    await page.goto(`/?demo=1&theme=${theme}`)
    const personal = page.getByRole('article', { name: 'Personal, Claude' })
    await expect(personal.locator('.model-chip')).toHaveCount(3)
    await personal.screenshot({ path: info.outputPath(`chips-personal-${theme}.png`) })
  })
}

test('expired credentials preserve a labeled last-known reading', async ({ page }) => {
  await page.goto('/?demo=1&state=stale')
  const account = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(account.getByText('Last known', { exact: true })).toBeVisible()
  await expect(account.getByText(/Updated 32m ago/)).toBeVisible()
  await expect(account.getByText(/session has expired/)).toBeVisible()
  await expect(account.getByRole('progressbar', { name: 'Session remaining, last known', exact: true })).toHaveAttribute('aria-valuenow', '72')
  await expect(account.getByRole('progressbar', { name: 'Session remaining, last known', exact: true })).toHaveAttribute('aria-valuetext', '72 percent remaining, last known')
})

test('signed-out accounts keep the last reading and offer the sign-in command', async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (text: string) => { (window as unknown as { copied: string }).copied = text } } })
  })
  await page.goto('/?demo=1&state=signed-out')
  const claude = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(claude.getByText('Last known', { exact: true })).toBeVisible()
  await expect(claude.locator('.stale-note')).toHaveText('Signed out 2h ago. Run:')
  await expect(claude.getByText(/Updated/)).toHaveCount(0)
  await expect(claude.getByRole('progressbar', { name: 'Session remaining, last known', exact: true })).toHaveAttribute('aria-valuenow', '72')
  const command = claude.getByRole('button', { name: 'Copy command: claude', exact: true })
  await expect(command).toHaveText('claude')
  await expect(command.locator('code')).toHaveCSS('font-family', /mono/i)
  await command.click()
  const copied = claude.getByRole('button', { name: 'Command copied' })
  await expect(copied).toHaveText('claudeCopied')
  expect(await page.evaluate(() => (window as unknown as { copied: string }).copied)).toBe('claude')
  await expect(copied).toHaveCount(0, { timeout: 3000 })
  const codex = page.getByRole('article', { name: 'Work, Codex' })
  await expect(codex.getByText('Signed out. Run:', { exact: true })).toBeVisible()
  await expect(codex.locator('[role="progressbar"]')).toHaveCount(0)
  await expect(codex.getByRole('button', { name: 'Copy command: CODEX_HOME=~/.codex-work codex login', exact: true })).toHaveText('CODEX_HOME=~/.codex-work codex login')
  await expect(codex.getByRole('button', { name: /Try again/ })).toBeVisible()
})

test('rate-limited and switched accounts retry without asking for attention', async ({ page }, info) => {
  await page.goto('/?demo=1&state=retrying&theme=dark')
  const limited = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(limited.getByRole('status').filter({ hasText: /^Rate limited · retrying in [34]m$/ })).toBeVisible()
  await expect(limited.getByText('Last known', { exact: true })).toHaveCount(0)
  await expect(limited.getByRole('progressbar', { name: 'Session remaining', exact: true })).toHaveAttribute('aria-valuenow', '72')
  const changed = page.getByRole('article', { name: 'Work, Codex' })
  await expect(changed.getByText('Retrying…', { exact: true })).toBeVisible()
  await expect(changed.getByText('Last known', { exact: true })).toHaveCount(0)
  const old = page.getByRole('article', { name: 'Studio, Claude' })
  await expect(old.getByText('Last known', { exact: true })).toBeVisible()
  await expect(old.getByText(/Updated 32m ago/)).toBeVisible()
  await expect(old.getByText(/retrying in/)).toHaveCount(0)
  await expect(page.getByText('1 account needs attention', { exact: true })).toBeVisible()
  await info.attach('retrying-dark', { body: await page.screenshot(), contentType: 'image/png' })
})

test('account without a reading shows no invented percentage', async ({ page }) => {
  await page.goto('/?demo=1&state=error')
  const account = page.getByRole('article', { name: 'Personal, Claude' })
  await expect(account.getByText('Usage unavailable', { exact: true })).toBeVisible()
  await expect(account.locator('[role="progressbar"]')).toHaveCount(0)
})

test('empty-state discovery is explicitly a preview', async ({ page }) => {
  await page.goto('/?demo=1&state=empty')
  await expect(page.getByRole('heading', { name: 'Add your accounts' })).toBeVisible()
  await page.getByRole('button', { name: 'Find accounts' }).click()
  await expect(page.getByRole('article')).toHaveCount(3)
  await expect(page.getByText('Sample accounts found.')).toBeVisible()
})

test('settings save theme and notification preferences', async ({ page }, info) => {
  await page.goto('/?demo=1&theme=dark')
  await page.getByRole('button', { name: 'Open settings' }).click()
  await page.getByRole('button', { name: 'Light', exact: true }).click()
  await expect(page.locator('html')).toHaveAttribute('data-theme', 'light')
  await page.getByRole('switch', { name: 'Usage alerts', exact: true }).click()
  await expect(page.getByRole('switch', { name: 'Usage alerts', exact: true })).toHaveAttribute('aria-checked', 'true')
  await page.getByRole('combobox', { name: 'Refresh while open', exact: true }).click()
  await page.getByRole('option', { name: '2 minutes', exact: true }).click()
  await expect(page.getByRole('combobox', { name: 'Refresh while open', exact: true })).toContainText('2 minutes')
  await info.attach('settings-light', { body: await page.screenshot(), contentType: 'image/png' })
})

test('removing an account requires the in-app confirmation', async ({ page }) => {
  await page.goto('/?demo=1')
  await page.getByRole('button', { name: 'Open settings' }).click()
  await page.getByRole('tab', { name: /^Accounts/ }).click()
  await page.getByRole('button', { name: 'Remove Personal', exact: true }).click()
  const dialog = page.getByRole('alertdialog')
  await expect(dialog).toBeVisible()
  await expect(page.getByRole('button', { name: 'Keep account' })).toBeFocused()
  await page.keyboard.press('Escape')
  await expect(dialog).toHaveCount(0)
  await page.getByRole('button', { name: 'Remove Personal', exact: true }).click()
  await dialog.getByRole('button', { name: 'Remove account', exact: true }).click()
  await expect(dialog).toHaveCount(0)
  await expect(page.getByRole('heading', { name: 'Personal', exact: true })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Add account', exact: true })).toBeFocused()
})

test('small displays retain navigation without horizontal overflow', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 480 })
  await page.goto('/?demo=1')
  await expect(page.getByRole('button', { name: 'Open settings' })).toBeVisible()
  await expect(page.getByRole('button', { name: 'Add account', exact: true })).toBeVisible()
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true)
})

test('unreadable settings offer a backed-up reset', async ({ page }, info) => {
  await page.goto('/?demo=1&state=recovery')
  const banner = page.getByRole('alert').filter({ hasText: 'Settings file couldn’t be read. Reset keeps a backup next to it.' })
  await expect(banner).toBeVisible()
  await expect(banner.getByText(/state\.json/)).toBeVisible()
  await info.attach('recovery', { body: await page.screenshot(), contentType: 'image/png' })
  await banner.getByRole('button', { name: 'Back up and reset settings' }).click()
  await expect(banner).toHaveCount(0)
  await expect(page.getByRole('article')).toHaveCount(3)
  await expect(page.getByText('Settings reset. The old file was kept as a backup.')).toBeVisible()
})

test('skipped accounts are noted in settings', async ({ page }) => {
  await page.goto('/?demo=1&state=recovered')
  await page.getByRole('button', { name: 'Open settings' }).click()
  await expect(page.getByRole('status').filter({ hasText: 'Skipped account “Old work” because its saved data couldn’t be read. A backup is kept next to the settings file.' })).toBeVisible()
})

test('accounts start right under the header', async ({ page }) => {
  await page.goto('/?demo=1')
  const shellTop = await page.locator('.app-shell').evaluate(element => element.getBoundingClientRect().top)
  const cardTop = await page.getByRole('article').first().evaluate(element => element.getBoundingClientRect().top)
  expect(cardTop - shellTop).toBeLessThan(72)
  const header = page.locator('.app-header')
  expect((await header.boundingBox())?.height).toBeLessThanOrEqual(56)
})

test('provider filters only appear when both providers have accounts', async ({ page }) => {
  await page.goto('/?demo=1&state=claude-only')
  await expect(page.getByRole('article')).toHaveCount(2)
  await expect(page.getByRole('group', { name: 'Filter by provider' })).toHaveCount(0)
})
