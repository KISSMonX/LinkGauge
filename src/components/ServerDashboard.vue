<script setup lang="ts">
import { computed } from 'vue'
import type { MetricPoint } from '../types'
import BandwidthChart from './BandwidthChart.vue'

/** 服务端独立概览：展示服务端自身的运行状态与观测统计，与客户端测试数据互不影响；
 *  对端为最近一次连接服务端的客户端 */
const props = defineProps<{ bindTarget: string; port: number; running: boolean; uptime: number; completed: number; serving: boolean; points: MetricPoint[]; peerIp: string; peerPort: number }>()
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
    <h2 class="panel-title">服务端概览</h2>
    <div class="network-grid server-status-grid">
      <section><h3>监听地址</h3><dl><dt>绑定 IP</dt><dd>{{ bindTarget || '所有网卡' }}</dd><dt>端口</dt><dd>{{ port }}</dd></dl></section>
      <section class="peer-section"><h3>对端客户端</h3><dl><dt>客户端 IP</dt><dd>{{ peerIp || '--（等待连接）' }}</dd><dt>端口</dt><dd>{{ peerPort || '--' }}</dd><dt>角色</dt><dd>客户端</dd></dl></section>
      <section><h3>运行状态</h3><dl><dt>状态</dt><dd><span :class="['status-pill', running ? 'ok' : 'idle']">{{ running ? '运行中' : '未运行' }}</span></dd><dt>已运行</dt><dd>{{ formatTime(uptime) }}</dd></dl></section>
      <section><h3>累计统计</h3><dl><dt>完成测试</dt><dd>{{ completed }} 次</dd><dt>当前</dt><dd>{{ serving ? '测试进行中' : '空闲' }}</dd></dl></section>
    </div>
    <section class="chart-card">
      <h3>{{ running ? '实时带宽（服务端观测）' : '带宽曲线（服务端观测）' }}</h3>
      <div class="chart-content"><div class="chart"><BandwidthChart :points="points" :live="running" /></div><dl class="metrics">
        <dt>当前带宽</dt><dd class="blue">{{ currentBandwidth.toFixed(2) }} Mbps</dd>
        <dt>平均带宽</dt><dd>{{ avgBandwidth.toFixed(2) }} Mbps</dd>
        <dt>最大带宽</dt><dd>{{ maxBandwidth.toFixed(2) }} Mbps</dd>
        <dt>总传输</dt><dd>{{ totalTransfer.toFixed(2) }} MB</dd>
        <dt>抖动</dt><dd>{{ (last?.jitterMs ?? 0).toFixed(2) }} ms</dd>
        <dt>丢包率</dt><dd>{{ (last?.lossPercent ?? 0).toFixed(2) }}%</dd>
      </dl></div>
    </section>
  </main>
</template>
