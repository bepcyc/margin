<script setup lang="ts">
import { computed } from 'vue'
import { burnThresholds, type Settings } from '../../lib/model'
import AppIcon from '../AppIcon.vue'
import AppSwitch from '../ui/AppSwitch.vue'

const props = defineProps<{ settings: Settings; busy: boolean }>()
const emit = defineEmits<{ change: [patch: Partial<Settings>] }>()
const presets = [50, 80, 90, 95, 100]
const choices = computed(() => [...new Set([...presets, ...props.settings.thresholds])].sort((a, b) => a - b))
const systemSettings = navigator.userAgent.includes('Windows') ? 'Windows Settings' : 'System Settings'
const blocked = computed(() => props.settings.notifications && props.settings.notificationPermission === 'denied')

const remaining = (threshold: number): number => 100 - threshold

function selected(threshold: number): boolean {
  return props.settings.thresholds.includes(threshold)
}

function locked(threshold: number): boolean {
  return selected(threshold) && props.settings.thresholds.length === 1
}

function toggle(threshold: number): void {
  if (locked(threshold)) return
  const thresholds = selected(threshold)
    ? props.settings.thresholds.filter(value => value !== threshold)
    : [...props.settings.thresholds, threshold].sort((a, b) => a - b)
  emit('change', { thresholds })
}

function setBurnThreshold(burnThreshold: Settings['burnThreshold']): void {
  if (burnThreshold !== props.settings.burnThreshold) emit('change', { burnThreshold })
}
</script>

<template>
  <section class="settings-section">
    <h2>Notifications</h2>
    <div class="setting-row"><span id="notification-label" class="setting-label">Usage alerts</span><AppSwitch :model-value="settings.notifications" aria-labelledby="notification-label" :disabled="busy" @update:model-value="emit('change', { notifications: $event })"/></div>
    <p v-if="blocked" class="notification-blocked" role="status"><AppIcon name="alert" :size="13"/><span>Notifications are off in {{ systemSettings }}.</span></p>
    <div class="threshold-options" :class="{ 'is-disabled': !settings.notifications }"><span>Notify at</span><span v-for="(threshold, index) in choices" :key="threshold" class="threshold-item"><button class="threshold-chip" :class="{ selected: selected(threshold) }" :aria-pressed="selected(threshold)" :aria-label="`Notify at ${remaining(threshold)}% left`" :disabled="busy || !settings.notifications || locked(threshold)" @click="toggle(threshold)"><AppIcon v-if="selected(threshold)" name="check" :size="11"/>{{ remaining(threshold) }}%</button><span v-if="index === choices.length - 1">left</span></span></div>
    <div class="setting-row pace-row" :class="{ 'is-disabled': !settings.notifications }"><div><span id="pace-alert-label" class="setting-label">Run-out warnings</span><p>When a limit is on pace to run out before it resets.</p></div><AppSwitch :model-value="settings.paceAlerts" aria-labelledby="pace-alert-label" :disabled="busy || !settings.notifications" @update:model-value="emit('change', { paceAlerts: $event })"/></div>
    <div class="setting-row stale-row" :class="{ 'is-disabled': !settings.notifications }"><div><span id="stale-alert-label" class="setting-label">Notify when usage stops updating</span><p>When an account can’t be read for 30 minutes.</p></div><AppSwitch :model-value="settings.staleAlerts" aria-labelledby="stale-alert-label" :disabled="busy || !settings.notifications" @update:model-value="emit('change', { staleAlerts: $event })"/></div>
    <div class="setting-row reset-row" :class="{ 'is-disabled': !settings.notifications }"><div><span id="reset-alert-label" class="setting-label">Notify when limits reset</span><p>Session and weekly, after a usage alert.</p></div><AppSwitch :model-value="settings.notifyOnReset" aria-labelledby="reset-alert-label" :disabled="busy || !settings.notifications" @update:model-value="emit('change', { notifyOnReset: $event })"/></div>
    <div class="setting-row burn-row" :class="{ 'is-disabled': !settings.notifications }"><div><span id="burn-alert-label" class="setting-label">Fast usage alerts</span><p>When a limit uses {{ settings.burnThreshold }}% or more within an hour.</p></div><AppSwitch :model-value="settings.burnAlerts" aria-labelledby="burn-alert-label" :disabled="busy || !settings.notifications" @update:model-value="emit('change', { burnAlerts: $event })"/></div>
    <div v-if="settings.burnAlerts" class="threshold-options burn-options" :class="{ 'is-disabled': !settings.notifications }" role="group" aria-label="Fast usage threshold"><button v-for="value in burnThresholds" :key="value" class="threshold-chip" :class="{ selected: settings.burnThreshold === value }" :aria-pressed="settings.burnThreshold === value" :aria-label="`${value}% within an hour`" :disabled="busy || !settings.notifications" @click="setBurnThreshold(value)"><AppIcon v-if="settings.burnThreshold === value" name="check" :size="11"/>{{ value }}%</button></div>
    <div class="setting-row digest-row" :class="{ 'is-disabled': !settings.notifications }"><div><span id="digest-label" class="setting-label">Weekly digest</span><p>Mondays at 09:00: last week's peaks and limit hits.</p></div><AppSwitch :model-value="settings.weeklyDigest" aria-labelledby="digest-label" :disabled="busy || !settings.notifications" @update:model-value="emit('change', { weeklyDigest: $event })"/></div>
  </section>
</template>

<style scoped>
.notification-blocked { display: flex; align-items: center; gap: 6px; margin: 2px 0 0; font-size: 13px; color: var(--secondary); }
.notification-blocked>svg { flex-shrink: 0; color: var(--warning); }
.threshold-chip:disabled { cursor: default; }
.pace-row,.stale-row,.reset-row,.burn-row,.digest-row { margin-top: 13px; transition: opacity .18s; }
.pace-row.is-disabled,.stale-row.is-disabled,.reset-row.is-disabled,.burn-row.is-disabled,.digest-row.is-disabled { opacity: .5; }
.burn-options { margin-top: 10px; }
</style>
