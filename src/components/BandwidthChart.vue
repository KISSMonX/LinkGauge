<script setup lang="ts">
import { computed } from 'vue'
import { Line } from 'vue-chartjs'
import { Chart as ChartJS, CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler, type ChartOptions } from 'chart.js'
import type { MetricPoint } from '../types'

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler)
const props = defineProps<{ points: MetricPoint[]; live?: boolean }>()
// 实时模式只显示最近 30 个点；测试完成后显示全部数据点
const shown = computed(() => (props.live ? props.points.slice(-30) : props.points))
const data = computed(() => ({
  labels: shown.value.map((p) => p.second === 0 ? '现在' : `${p.second}s`),
  datasets: [{ data: shown.value.map((p) => p.bandwidthMbps), borderColor: '#1473e6', backgroundColor: 'rgba(20,115,230,.08)', fill: true, tension: .22, pointRadius: 1.6, borderWidth: 1.5 }]
}))
// 实时模式：y 轴从 0 开始，便于观察波动；完成模式：自动缩放适配整个测试过程的数据范围
const options = computed<ChartOptions<'line'>>(() => ({
  responsive: true, maintainAspectRatio: false, animation: false,
  plugins: { legend: { display: false }, tooltip: { displayColors: false } },
  scales: {
    x: { grid: { color: '#edf0f4' }, ticks: { color: '#606b7a', maxTicksLimit: props.live ? 7 : 12 } },
    y: props.live
      ? { beginAtZero: true, grid: { color: '#e5e9ef' }, ticks: { color: '#606b7a', callback: (v: string | number) => `${v} Mbps` } }
      : { grid: { color: '#e5e9ef' }, ticks: { color: '#606b7a', callback: (v: string | number) => `${v} Mbps` } }
  }
}))
</script>

<template><Line :data="data" :options="options" /></template>
