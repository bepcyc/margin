import { expect, test, type Page } from '@playwright/test'
import { defaultSettings } from '../../src/lib/model.ts'

type Placement = { revision: number; arrowX: number | null }
type PlacementHarness = {
  emit: (payload: unknown) => void
  completeRead: (payload: unknown) => void
  readStarted: boolean
  subscribedBeforeRead: boolean
}

declare global {
  interface Window { __PLACEMENT_TEST__?: PlacementHarness }
}

async function installNativeBridge(page: Page, options: { placement: Placement; delayRead?: boolean }): Promise<void> {
  await page.addInitScript(({ placement, delayRead, settings }) => {
    const handlers = new Map<string, (event: { payload: unknown }) => void>()
    let finishRead: ((payload: unknown) => void) | undefined
    const harness: PlacementHarness = {
      emit(payload) { handlers.get('popover_placement_changed')?.({ payload }) },
      completeRead(payload) { finishRead?.(payload) },
      readStarted: false,
      subscribedBeforeRead: false,
    }
    window.__PLACEMENT_TEST__ = harness
    window.__TAURI__ = {
      core: {
        async invoke(command) {
          if (command === 'get_state') return { revision: 0, accounts: [], settings, refreshing: false, notice: null }
          if (command === 'get_popover_placement') {
            harness.readStarted = true
            harness.subscribedBeforeRead = handlers.has('popover_placement_changed')
            if (delayRead) return new Promise<unknown>(resolve => { finishRead = resolve })
            return placement
          }
          return undefined
        },
      },
      event: {
        async listen(event, handler) {
          handlers.set(event, handler)
          return () => { handlers.delete(event) }
        },
      },
    }
  }, { ...options, settings: defaultSettings })
}

test.describe('Mac popover placement', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)' })

  test('native arrow follows the tray and ignores a delayed older placement read', async ({ page }, info) => {
    await installNativeBridge(page, { placement: { revision: 0, arrowX: null }, delayRead: true })
    await page.goto('/')
    await expect.poll(() => page.evaluate(() => window.__PLACEMENT_TEST__?.readStarted)).toBe(true)
    expect(await page.evaluate(() => window.__PLACEMENT_TEST__?.subscribedBeforeRead)).toBe(true)
    await expect(page.locator('.popover-arrow')).toHaveCount(0)

    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 2, arrowX: 340 }))
    const arrow = page.locator('.popover-arrow')
    await expect(arrow).toHaveCSS('left', '334px')
    await expect(arrow).toHaveCSS('top', '9px')
    const bounds = await arrow.boundingBox()
    expect(bounds).not.toBeNull()
    expect((bounds?.x ?? 0) + (bounds?.width ?? 0) / 2).toBeCloseTo(340, 1)
    expect(bounds?.y).toBeCloseTo(6.515, 1)
    expect(await page.locator('.popover').boundingBox()).toEqual({ x: 6, y: 14, width: 408, height: 640 })
    await info.attach('native-arrow-off-center', { body: await page.screenshot(), contentType: 'image/png' })

    await page.evaluate(async () => {
      window.__PLACEMENT_TEST__?.completeRead({ revision: 1, arrowX: 80 })
      await new Promise<void>(resolve => requestAnimationFrame(() => resolve()))
    })
    await expect(arrow).toHaveCSS('left', '334px')
    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 3, arrowX: null }))
    await expect(arrow).toHaveCount(0)
    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 2, arrowX: 210 }))
    await expect(arrow).toHaveCount(0)
    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 4, arrowX: 132 }))
    await expect(arrow).toHaveCSS('left', '126px')
  })

  test('unsafe anchors hide the arrow until a reliable placement arrives', async ({ page }) => {
    await installNativeBridge(page, { placement: { revision: 1, arrowX: 10 } })
    await page.goto('/')
    await expect.poll(() => page.evaluate(() => window.__PLACEMENT_TEST__?.readStarted)).toBe(true)
    await expect(page.locator('.popover-arrow')).toHaveCount(0)
    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 2, arrowX: 394 }))
    await expect(page.locator('.popover-arrow')).toHaveCSS('left', '388px')
    await page.setViewportSize({ width: 360, height: 660 })
    await expect(page.locator('.popover-arrow')).toHaveCount(0)
    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 3, arrowX: 26 }))
    await expect(page.locator('.popover-arrow')).toHaveCSS('left', '20px')
    await page.evaluate(() => window.__PLACEMENT_TEST__?.emit({ revision: 4, arrowX: 'unknown' }))
    await expect(page.locator('.popover-arrow')).toHaveCount(0)
  })
})

test.describe('Windows popover placement', () => {
  test.use({ userAgent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64)' })

  test('Windows keeps a rounded panel without a tray arrow', async ({ page }) => {
    await installNativeBridge(page, { placement: { revision: 1, arrowX: 210 } })
    await page.goto('/')
    await expect.poll(() => page.evaluate(() => window.__PLACEMENT_TEST__?.readStarted)).toBe(true)
    await expect(page.locator('.popover-arrow')).toHaveCount(0)
    await expect(page.locator('.popover')).toHaveCSS('border-radius', '8px')
    expect(await page.locator('.popover').boundingBox()).toEqual({ x: 6, y: 6, width: 408, height: 648 })
  })
})

test.describe('Linux popover placement', () => {
  test.use({ userAgent: 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 Chrome/130.0.0.0 Safari/537.36' })

  test('Linux keeps a rounded panel without a tray arrow', async ({ page }) => {
    await installNativeBridge(page, { placement: { revision: 1, arrowX: 210 } })
    await page.goto('/')
    await expect.poll(() => page.evaluate(() => window.__PLACEMENT_TEST__?.readStarted)).toBe(true)
    await expect(page.locator('.popover-arrow')).toHaveCount(0)
    await expect(page.locator('.popover')).toHaveCSS('border-radius', '8px')
    expect(await page.locator('.popover').boundingBox()).toEqual({ x: 6, y: 6, width: 408, height: 648 })
  })
})
