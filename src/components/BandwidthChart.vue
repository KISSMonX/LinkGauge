<script setup lang="ts">
import { computed } from 'vue'
import { Line } from 'vue-chartjs'
import { Chart as ChartJS, CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler, type ChartOptions } from 'chart.js'
import type { MetricPoint } from '../types'
import { useI18n } from '../i18n'
import { theme } from '../theme'

ChartJS.register(CategoryScale, LinearScale, PointElement, LineElement, Tooltip, Filler)
const props = defineProps<{ points: MetricPoint[]; live?: boolean }>()
const { t } = useI18n()
/** 主题色从 CSS 变量读取；colors 依赖 theme ref，切换主题后图表自动重绘 */
const cssVar = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim()
const colors = computed(() => {
  void theme.value
  return {
    line: cssVar('--chart-line'),
    fill: cssVar('--chart-fill'),
    grid: cssVar('--chart-grid'),
    gridLight: cssVar('--chart-grid-light'),
    tick: cssVar('--chart-tick'),
  }
})
// 实时模式只显示最近 30 个点；测试完成后显示全部数据点
const shown = computed(() => (props.live ? props.points.slice(-30) : props.points))
const data = computed(() => ({
  labels: shown.value.map((p) => p.second === 0 ? t('chart.now') : `${p.second}s`),
  datasets: [{ data: shown.value.map((p) => p.bandwidthMbps), borderColor: colors.value.line, backgroundColor: colors.value.fill, fill: true, tension: .22, pointRadius: 1.6, borderWidth: 1.5 }]
}))
// 实时模式：y 轴从 0 开始，便于观察波动；完成模式：自动缩放适配整个测试过程的数据范围
const options = computed<ChartOptions<'line'>>(() => ({
  responsive: true, maintainAspectRatio: false, animation: false,
  plugins: { legend: { display: false }, tooltip: { displayColors: false } },
  scales: {
    x: { grid: { color: colors.value.gridLight }, ticks: { color: colors.value.tick, maxTicksLimit: props.live ? 7 : 12 } },
    y: {
      beginAtZero: props.live,
      grid: { color: colors.value.grid },
      ticks: { color: colors.value.tick, callback: (v: string | number) => `${v} ${t('chart.mbps')}` }
    }
  }
}))
</script>

<template><Line :data="data" :options="options" /></template>
