<script setup lang="ts">
import type { TestConfig, TestSummary } from '../types'
import Icon from './Icon.vue'

defineProps<{ config: TestConfig; summary: TestSummary }>()
const emit = defineEmits<{ report: [format: 'html' | 'pdf'] }>()
</script>

<template>
  <footer class="panel report-summary">
    <h2 class="panel-title">测试报告概览</h2>
    <div class="summary-grid">
      <div><span>测试时间</span><strong>{{ summary.startedAt || '--' }}</strong></div>
      <div><span>测试模式</span><strong>{{ config.mode === 'client' ? '客户端' : '服务端' }} / {{ config.protocol.toUpperCase() }}</strong></div>
      <div><span>已完成项目数</span><strong>{{ summary.completed }} / {{ summary.total }}</strong></div>
      <div><span>平均带宽</span><strong class="blue">{{ summary.averageBandwidth.toFixed(2) }} Mbps</strong></div>
      <div><span>Ping 平均时延</span><strong class="green">{{ summary.pingAverage.toFixed(2) }} ms</strong></div>
      <div><span>丢包率</span><strong class="green">{{ summary.lossPercent.toFixed(2) }}%</strong></div>
      <div class="report-output"><span>报告输出</span><p><button @click="emit('report','html')">🌐 HTML</button><button class="pdf" @click="emit('report','pdf')"><Icon name="report" />PDF</button></p></div>
    </div>
  </footer>
</template>
