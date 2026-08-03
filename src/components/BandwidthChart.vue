<script setup lang="ts">
import { computed } from 'vue'
import { Line } from 'vue-chartjs'
import { Chart as ChartJS, CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler } from 'chart.js'
import type { MetricPoint } from '../types'

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler)
const props = defineProps<{ points: MetricPoint[] }>()
const recent = computed(() => props.points.slice(-30))
const data = computed(() => ({
  labels: recent.value.map((p) => p.second === 0 ? '现在' : `${p.second}s`),
  datasets: [{ data: recent.value.map((p) => p.bandwidthMbps), borderColor: '#1473e6', backgroundColor: 'rgba(20,115,230,.08)', fill: true, tension: .22, pointRadius: 1.6, borderWidth: 1.5 }]
}))
const options = {
  responsive: true, maintainAspectRatio: false, animation: false,
  plugins: { legend: { display: false }, tooltip: { displayColors: false } },
  scales: {
    x: { grid: { color: '#edf0f4' }, ticks: { color: '#606b7a', maxTicksLimit: 7 } },
    y: { beginAtZero: true, grid: { color: '#e5e9ef' }, ticks: { color: '#606b7a', callback: (v: string | number) => `${v} Mbps` } }
  }
} as const
</script>

<template><Line :data="data" :options="options" /></template>
