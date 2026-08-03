<script setup lang="ts">
import type { TestConfig, TestSummary } from '../types'
import { useI18n } from '../i18n'
import Icon from './Icon.vue'

defineProps<{ config: TestConfig; summary: TestSummary }>()
const emit = defineEmits<{ report: [format: 'html' | 'pdf']; 'open-dir': [] }>()
const { t } = useI18n()
</script>

<template>
  <footer class="panel report-summary">
    <h2 class="panel-title">{{ t('rep.title') }}</h2>
    <div class="summary-grid">
      <div><span>{{ t('rep.time') }}</span><strong>{{ summary.startedAt || '--' }}</strong></div>
      <div><span>{{ t('rep.mode') }}</span><strong>{{ config.mode === 'client' ? t('common.client') : t('common.server') }} / {{ t('rep.mixed') }}</strong></div>
      <div><span>{{ t('rep.completed') }}</span><strong>{{ summary.completed }} / {{ summary.total }}</strong></div>
      <div><span>{{ t('rep.avgBw') }}</span><strong class="blue">{{ summary.averageBandwidth.toFixed(2) }} Mbps</strong></div>
      <div><span>{{ t('rep.ping') }}</span><strong class="green">{{ summary.pingAverage.toFixed(2) }} ms</strong></div>
      <div><span>{{ t('rep.loss') }}</span><strong class="green">{{ summary.lossPercent.toFixed(2) }}%</strong></div>
      <div class="report-output"><span>{{ t('rep.output') }}</span><p><button @click="emit('report','html')">🌐 HTML</button><button class="pdf" @click="emit('report','pdf')"><Icon name="report" />PDF</button><button @click="emit('open-dir')">📁 {{ t('rep.openDir') }}</button></p></div>
    </div>
  </footer>
</template>
