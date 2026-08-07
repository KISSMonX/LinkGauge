<script setup lang="ts">
import { computed } from 'vue'
import type { NetworkInfo, ServerConfig, TestConfig, TestItem, MetricPoint, TestSummary } from '../types'
import { itemLabel, useI18n } from '../i18n'
import { formatBandwidth, formatTime, formatTransfer } from '../format'
import BandwidthChart from './BandwidthChart.vue'

const props = defineProps<{ local: NetworkInfo; config: TestConfig; serverConfig: ServerConfig; current?: TestItem; points: MetricPoint[]; progress: number; elapsed: number; summary: TestSummary; connected: boolean; serverRunning: boolean; live?: boolean; completedLabel?: string; localPort?: number }>()
const { t } = useI18n()
const currentBandwidth = computed(() => props.points.at(-1)?.bandwidthMbps || 0)
const remain = computed(() => Math.max(0, props.config.duration - props.elapsed))
const chartTitle = computed(() => {
  const label = props.current ? itemLabel(props.current.id) : (props.completedLabel || t('cfg.item.tcp-single'))
  return props.live ? t('dash.liveChart', { label }) : t('dash.doneChart', { label })
})
</script>

<template>
  <main class="panel dashboard">
    <h2 class="panel-title">{{ t('dash.title') }}</h2>
    <div class="network-grid">
      <section><h3>{{ t('dash.local') }}</h3><dl><dt>{{ t('dash.ip') }}</dt><dd>{{ local.ip }}</dd><dt>{{ t('dash.nic') }}</dt><dd>{{ local.interfaceName }}</dd><dt>{{ t('dash.nicSpeed') }}</dt><dd :title="local.speedMbps ? `${local.speedMbps} Mbps` : undefined">{{ local.speedMbps ? formatBandwidth(local.speedMbps) : t('dash.unknown') }}</dd><dt>{{ t('dash.mac') }}</dt><dd>{{ local.mac }}</dd><dt>{{ t('dash.hostname') }}</dt><dd>{{ local.hostname }}</dd></dl></section>
      <section><h3>{{ t('dash.peer') }}</h3><dl><dt>{{ t('dash.ip') }}</dt><dd>{{ config.serverIp }}</dd><dt>{{ t('dash.port') }}</dt><dd>{{ config.port }}</dd><dt>{{ t('dash.mac') }}</dt><dd>{{ t('dash.autoProbe') }}</dd><dt>{{ t('dash.role') }}</dt><dd>{{ config.mode === 'client' ? t('common.server') : t('common.client') }}</dd></dl></section>
      <section class="connection"><div class="section-title"><h3>{{ t('dash.conn') }}</h3><span :class="['status-pill', connected ? 'ok' : 'idle']">{{ connected ? t('dash.connected') : t('dash.disconnected') }}</span></div><dl><dt>{{ t('dash.rtt') }}</dt><dd>{{ summary.pingAverage ? summary.pingAverage.toFixed(2) : '--' }} ms</dd><dt>{{ t('dash.loss') }}</dt><dd>{{ summary.lossPercent.toFixed(2) }}%</dd><dt>{{ t('dash.localPort') }}</dt><dd>{{ localPort || '--' }}</dd><dt>{{ t('dash.connTime') }}</dt><dd>{{ formatTime(elapsed) }}</dd></dl></section>
    </div>
    <div class="current-row">
      <section class="current-test"><h3>{{ t('dash.current') }}</h3><strong>{{ current ? `${itemLabel(current.id)}` : (serverRunning ? t('dash.serverRunning') : t('dash.waitStart')) }}</strong><p>{{ current ? (current.protocol === 'ping' ? t('dash.pinging') : t('dash.testing')) : (serverRunning ? t('dash.serverListening', { port: serverConfig.port }) : t('dash.pickTest')) }}</p></section>
      <section class="overall-progress"><div class="section-title"><h3>{{ t('dash.progress') }}</h3><b>{{ progress }}%</b></div><div class="progress"><i :style="{ width: `${progress}%` }"></i></div><p>{{ t('dash.elapsed', { elapsed: formatTime(elapsed) }) }} / {{ t('dash.remain', { remain: formatTime(remain) }) }}</p></section>
    </div>
    <section class="chart-card">
      <h3>{{ chartTitle }}</h3>
      <div class="chart-content"><div class="chart"><BandwidthChart :points="points" :live="live" /></div><dl class="metrics"><dt :title="t('dash.currentBwTitle')">{{ t('dash.currentBw') }}</dt><dd class="blue" :title="`${currentBandwidth.toFixed(2)} Mbps`">{{ formatBandwidth(currentBandwidth) }}</dd><dt :title="t('dash.avgBwTitle')">{{ t('dash.avgBw') }}</dt><dd :title="`${summary.averageBandwidth.toFixed(2)} Mbps`">{{ formatBandwidth(summary.averageBandwidth) }}</dd><dt :title="t('dash.minBwTitle')">{{ t('dash.minBw') }}</dt><dd :title="`${summary.minBandwidth.toFixed(2)} Mbps`">{{ formatBandwidth(summary.minBandwidth) }}</dd><dt :title="t('dash.maxBwTitle')">{{ t('dash.maxBw') }}</dt><dd :title="`${summary.maxBandwidth.toFixed(2)} Mbps`">{{ formatBandwidth(summary.maxBandwidth) }}</dd><dt :title="t('dash.totalTransferTitle')">{{ t('dash.totalTransfer') }}</dt><dd :title="`${summary.totalTransferMb.toFixed(2)} MB`">{{ formatTransfer(summary.totalTransferMb) }}</dd><dt :title="t('dash.retransTitle')">{{ t('dash.retrans') }}</dt><dd>0.00%</dd></dl></div>
    </section>
  </main>
</template>
