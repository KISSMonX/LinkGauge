<script setup lang="ts">
import { computed } from 'vue'
import type { MetricPoint } from '../types'
import { useI18n } from '../i18n'
import BandwidthChart from './BandwidthChart.vue'

/** 服务端独立概览：展示服务端自身的运行状态与观测统计，与客户端测试数据互不影响；
 *  对端为最近一次连接服务端的客户端 */
const props = defineProps<{ bindTarget: string; port: number; running: boolean; uptime: number; completed: number; serving: boolean; points: MetricPoint[]; peerIp: string; peerPort: number }>()
const { t } = useI18n()
const currentBandwidth = computed(() => props.points.at(-1)?.bandwidthMbps || 0)
const formatTime = (s: number) => `00:${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
const avgBandwidth = computed(() => {
  const valid = props.points.filter((p) => p.bandwidthMbps > 0)
  return valid.length ? valid.reduce((a, b) => a + b.bandwidthMbps, 0) / valid.length : 0
})
const maxBandwidth = computed(() => {
  const valid = props.points.filter((p) => p.bandwidthMbps > 0)
  return valid.length ? Math.max(...valid.map((p) => p.bandwidthMbps)) : 0
})
const totalTransfer = computed(() => props.points.reduce((a, b) => a + b.transferMb, 0))
const last = computed(() => props.points.at(-1))
</script>

<template>
  <main class="panel dashboard">
    <h2 class="panel-title">{{ t('sdash.title') }}</h2>
    <div class="network-grid server-status-grid">
      <section><h3>{{ t('sdash.listen') }}</h3><dl><dt>{{ t('sdash.bindIp') }}</dt><dd>{{ bindTarget || t('sdash.allAdapters') }}</dd><dt>{{ t('sdash.port') }}</dt><dd>{{ port }}</dd></dl></section>
      <section class="peer-section"><h3>{{ t('sdash.peerClient') }}</h3><dl><dt>{{ t('sdash.clientIp') }}</dt><dd>{{ peerIp || t('sdash.waitingConn') }}</dd><dt>{{ t('sdash.port') }}</dt><dd>{{ peerPort || '--' }}</dd><dt>{{ t('dash.role') }}</dt><dd>{{ t('common.client') }}</dd></dl></section>
      <section><h3>{{ t('sdash.state') }}</h3><dl><dt>{{ t('sdash.status') }}</dt><dd><span :class="['status-pill', running ? 'ok' : 'idle']">{{ running ? t('common.running') : t('common.notRunning') }}</span></dd><dt>{{ t('sdash.uptime') }}</dt><dd>{{ formatTime(uptime) }}</dd></dl></section>
      <section><h3>{{ t('sdash.stats') }}</h3><dl><dt>{{ t('sdash.completed') }}</dt><dd>{{ t('sdash.completedValue', { n: completed }) }}</dd><dt>{{ t('sdash.current') }}</dt><dd>{{ serving ? t('common.testing') : t('common.idle') }}</dd></dl></section>
    </div>
    <section class="chart-card">
      <h3>{{ running ? t('sdash.liveChart') : t('sdash.doneChart') }}</h3>
      <div class="chart-content"><div class="chart"><BandwidthChart :points="points" :live="running" /></div><dl class="metrics">
        <dt>{{ t('dash.currentBw') }}</dt><dd class="blue">{{ currentBandwidth.toFixed(2) }} Mbps</dd>
        <dt>{{ t('dash.avgBw') }}</dt><dd>{{ avgBandwidth.toFixed(2) }} Mbps</dd>
        <dt>{{ t('dash.maxBw') }}</dt><dd>{{ maxBandwidth.toFixed(2) }} Mbps</dd>
        <dt>{{ t('dash.totalTransfer') }}</dt><dd>{{ totalTransfer.toFixed(2) }} MB</dd>
        <dt>{{ t('sdash.jitter') }}</dt><dd>{{ (last?.jitterMs ?? 0).toFixed(2) }} ms</dd>
        <dt>{{ t('dash.loss') }}</dt><dd>{{ (last?.lossPercent ?? 0).toFixed(2) }}%</dd>
      </dl></div>
    </section>
  </main>
</template>
