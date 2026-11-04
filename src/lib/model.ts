import { z } from 'zod'

export const providerSchema = z.enum(['claude', 'codex'])
export const accountIdSchema = z.string().min(1).brand<'AccountId'>()
export const burnThresholds = [10, 20, 30] as const
export const settingsSchema = z.object({
  theme: z.enum(['system', 'light', 'dark']),
  overviewLayout: z.enum(['cards', 'compact']).default('cards'),
  accountOrder: z.enum(['urgency', 'added']).default('urgency'),
  pollIntervalSeconds: z.number().int().min(30).max(3600),
  idlePollIntervalSeconds: z.number().int().min(30).max(86400),
  notifications: z.boolean(),
  thresholds: z.array(z.number().int().min(1).max(100)).min(1).max(10)
    .refine(values => values.every((value, index) => index === 0 || value > (values[index - 1] ?? 0)), 'Thresholds must be distinct and increasing'),
  notifyOnReset: z.boolean().default(false),
  paceAlerts: z.boolean().default(true),
  staleAlerts: z.boolean().default(true),
  burnAlerts: z.boolean().default(false),
  burnThreshold: z.literal(burnThresholds).default(20),
  weeklyDigest: z.boolean().default(false),
  notificationPermission: z.enum(['unasked', 'granted', 'denied']).default('unasked'),
  trayMode: z.enum(['pinned', 'lowest']),
  trayMetric: z.enum(['core', 'all', 'session', 'weekly']).default('core'),
  trayShowPercent: z.boolean().default(false),
  pinnedAccountId: accountIdSchema.nullable(),
  autostart: z.boolean(),
  autoCheckUpdates: z.boolean().default(true),
}).refine(value => value.idlePollIntervalSeconds >= value.pollIntervalSeconds, 'Background refresh must be slower than active refresh')
export const usageWindowSchema = z.object({
  id: z.string(),
  label: z.string(),
  usedPercent: z.number().min(0).max(100),
  resetsAt: z.string().datetime({ offset: true }).nullable(),
  windowSeconds: z.number().int().nonnegative().nullable(),
  scope: z.enum(['core', 'model', 'other']).optional(),
})
export const resetGrantsSchema = z.object({
  available: z.number().int().nonnegative(),
  kind: z.string().nullable(),
  expiresAt: z.string().datetime({ offset: true }).nullable(),
})
export const balanceSchema = z.object({
  credits: z.object({ amount: z.number().nonnegative().nullable(), unlimited: z.boolean() }).nullable().default(null),
  spendLimit: z.object({
    used: z.number().nonnegative(),
    limit: z.number().nonnegative(),
    usedPercent: z.number().min(0).max(100),
    resetsAt: z.string().datetime({ offset: true }).nullable(),
    reached: z.boolean(),
    unit: z.string().nullable().default(null),
  }).nullable().default(null),
})
export const accountSchema = z.object({
  id: accountIdSchema,
  provider: providerSchema,
  label: z.string(),
  color: z.string(),
  source: z.discriminatedUnion('kind', [
    z.object({ kind: z.literal('configDir'), path: z.string(), keychainSuffix: z.string().optional() }),
    z.object({ kind: z.literal('keychain') }),
  ]),
  identity: z.object({ email: z.string().nullable(), plan: z.string().nullable() }),
  snapshot: z.object({
    windows: z.array(usageWindowSchema),
    fetchedAt: z.string().datetime({ offset: true }),
    extraUsage: z.object({
      spent: z.number().nonnegative(),
      limit: z.number().nonnegative().nullable(),
      currency: z.string(),
    }).nullable(),
    resetGrants: resetGrantsSchema.nullable().default(null),
    balance: balanceSchema.nullable().default(null),
  }).nullable(),
  error: z.object({
    kind: z.string(),
    message: z.string(),
    retryAfterSeconds: z.number().nonnegative().nullable(),
    signedOutSince: z.string().datetime({ offset: true }).optional(),
    signInCommand: z.string().optional(),
    retryAt: z.number().int().nonnegative().optional(),
  }).nullable(),
  refreshing: z.boolean(),
})
export const restoreOfferSchema = z.object({
  id: z.string(),
  provider: providerSchema,
  reason: z.enum(['unfinished', 'verifyFailed', 'rollbackFailed']),
  createdAt: z.string(),
})
export const switchViewSchema = z.object({
  activeAccountIds: z.array(z.string()).default([]),
  restore: restoreOfferSchema.nullable().default(null),
})
export const makeActivePreviewSchema = z.object({
  accountId: accountIdSchema,
  provider: providerSchema,
  kind: z.enum(['swap', 'move']),
  fingerprint: z.string(),
  target: z.string(),
  displaced: z.string().nullable(),
  folder: z.string(),
  running: z.boolean(),
})
export const popoverShownSchema = z.object({ hiddenForMs: z.number().nonnegative().nullable() })
export const appStateSchema = z.object({
  revision: z.number().int().nonnegative(),
  accounts: z.array(accountSchema),
  settings: settingsSchema,
  refreshing: z.boolean(),
  notice: z.string().nullable().default(null),
  bridgeNotice: z.string().nullable().default(null),
  recovery: z.boolean().default(false),
  recovered: z.array(z.string()).default([]),
  ignoredSources: z.array(z.string()).default([]),
  switching: switchViewSchema.default({ activeAccountIds: [], restore: null }),
})
export const activatedSchema = z.object({
  state: appStateSchema,
  target: z.string(),
  previousAccountId: accountIdSchema.nullable(),
})

export type Provider = z.infer<typeof providerSchema>
export type AccountId = z.infer<typeof accountIdSchema>
export type Account = z.infer<typeof accountSchema>
export type Settings = z.infer<typeof settingsSchema>
export type UsageWindow = z.infer<typeof usageWindowSchema>
export type ResetGrants = z.infer<typeof resetGrantsSchema>
export type Balance = z.infer<typeof balanceSchema>
export type WindowScope = NonNullable<UsageWindow['scope']>
export type AppState = z.infer<typeof appStateSchema>
export type RestoreOffer = z.infer<typeof restoreOfferSchema>

/** Restores are offered for 30 minutes; older backups may hold used tokens. */
export function liveRestoreOffer(offer: RestoreOffer | null, now: number): RestoreOffer | null {
  if (!offer) return null
  const age = now - Date.parse(offer.createdAt)
  return age >= 0 && age < 30 * 60_000 ? offer : null
}
export type MakeActivePreview = z.infer<typeof makeActivePreviewSchema>
export type PopoverShown = z.infer<typeof popoverShownSchema>

export const defaultSettings: Settings = {
  theme: 'system',
  overviewLayout: 'cards',
  accountOrder: 'urgency',
  pollIntervalSeconds: 60,
  idlePollIntervalSeconds: 300,
  notifications: true,
  thresholds: [80, 95, 100],
  notifyOnReset: false,
  paceAlerts: true,
  staleAlerts: true,
  burnAlerts: false,
  burnThreshold: 20,
  weeklyDigest: false,
  notificationPermission: 'unasked',
  trayMode: 'lowest',
  trayMetric: 'core',
  trayShowPercent: false,
  pinnedAccountId: null,
  autostart: false,
  autoCheckUpdates: true,
}
