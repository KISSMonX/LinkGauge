<script setup lang="ts">
import { computed } from 'vue'
import type { NetworkInfo, ServerConfig, TestConfig, TestItem, MetricPoint, TestSummary } from '../types'
import BandwidthChart from './BandwidthChart.vue'

const props = defineProps<{ local: NetworkInfo; config: TestConfig; serverConfig: ServerConfig; current?: TestItem; points: MetricPoint[]; progress: number; elapsed: number; summary: TestSummary; connected: boolean; serverRunning: boolean; live?: boolean; completedLabel?: string; localPort?: number }>()
const currentBandwidth = computed(() => props.points.at(-1)?.bandwidthMbps || 0)
const remain = computed(() => Math.max(0, props.config.duration - props.elapsed))
const formatTime = (s: number) => `00:${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
const chartTitle = computed(() => {
  const label = props.current?.label || props.completedLabel || 'TCP 单向带宽'
  return props.live ? `实时带宽（${label}）` : `带宽曲线 · 完整过程（${label}）`
})
</script>

<template>
  <main class="panel dashboard">
    <h2 class="panel-title">当前测试概览</h2>
    <div class="network-grid">
      <section><h3>本机网络信息</h3><dl><dt>IP 地址</dt><dd>{{ local.ip }}</dd><dt>网卡名称</dt><dd>{{ local.interfaceName }}</dd><dt>网卡带宽</dt><dd>{{ local.speedMbps ? local.speedMbps + ' Mbps' : '未知' }}</dd><dt>MAC 地址</dt><dd>{{ local.mac }}</dd><dt>主机名</dt><dd>{{ local.hostname }}</dd></dl></section>
      <section><h3>对端网络信息</h3><dl><dt>IP 地址</dt><dd>{{ config.serverIp }}</dd><dt>端口</dt><dd>{{ config.port }}</dd><dt>MAC 地址</dt><dd>自动探测</dd><dt>角色</dt><dd>{{ config.mode === 'client' ? '服务端' : '客户端' }}</dd></dl></section>
      <section class="connection"><div class="section-title"><h3>连接状态</h3><span :class="['status-pill', connected ? 'ok' : 'idle']">{{ connected ? '已连接' : '未连接' }}</span></div><dl><dt>RTT</dt><dd>{{ summary.pingAverage ? summary.pingAverage.toFixed(2) : '--' }} ms</dd><dt>丢包率</dt><dd>{{ summary.lossPercent.toFixed(2) }}%</dd><dt>本地端口</dt><dd>{{ localPort || '--' }}</dd><dt>连接时间</dt><dd>{{ formatTime(elapsed) }}</dd></dl></section>
    </div>
    <div class="current-row">
      <section class="current-test"><h3>当前执行项</h3><strong>{{ current ? `${current.label}` : (serverRunning ? 'riperf3 服务端运行中' : '等待开始测试') }}</strong><p>{{ current ? (current.protocol === 'ping' ? '正在检测网络连通性' : `正在测试（客户端 → 服务端）`) : (serverRunning ? `监听端口 ${serverConfig.port}，可同时运行客户端测试` : '请选择测试项目并开始') }}</p></section>
      <section class="overall-progress"><div class="section-title"><h3>总体进度</h3><b>{{ progress }}%</b></div><div class="progress"><i :style="{ width: `${progress}%` }"></i></div><p>已用时：{{ formatTime(elapsed) }} / 预计剩余：{{ formatTime(remain) }}</p></section>
    </div>
    <section class="chart-card">
      <h3>{{ chartTitle }}</h3>
      <div class="chart-content"><div class="chart"><BandwidthChart :points="points" :live="live" /></div><dl class="metrics"><dt>当前带宽</dt><dd class="blue">{{ currentBandwidth.toFixed(2) }} Mbps</dd><dt>平均带宽</dt><dd>{{ summary.averageBandwidth.toFixed(2) }} Mbps</dd><dt>最小带宽</dt><dd>{{ summary.minBandwidth.toFixed(2) }} Mbps</dd><dt>最大带宽</dt><dd>{{ summary.maxBandwidth.toFixed(2) }} Mbps</dd><dt>总传输</dt><dd>{{ summary.totalTransferMb.toFixed(2) }} MB</dd><dt>重传率</dt><dd>0.00%</dd></dl></div>
    </section>
  </main>
</template>
