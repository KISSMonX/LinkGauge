<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { ItemHistory, LogEntry, TestItem } from '../types'
import { itemLabel, useI18n } from '../i18n'
import Icon from './Icon.vue'

const props = defineProps<{ items: TestItem[]; logs: LogEntry[]; serverRunning: boolean; mode?: 'full' | 'logs'; history?: Record<string, ItemHistory>; historySelected?: string; running?: boolean }>()
const emit = defineEmits<{ clear: []; 'open-log-dir': []; report: []; select: [id: string] }>()
const { t } = useI18n()
/** logs 模式（服务端分离窗口）只显示日志，隐藏客户端执行队列与报告区 */
const showQueue = computed(() => props.mode !== 'logs')
const filter = ref<'ALL' | 'INFO' | 'WARN' | 'ERROR'>('ALL')
const logBox = ref<HTMLElement>()
const visibleLogs = computed(() => filter.value === 'ALL' ? props.logs : props.logs.filter((l) => l.level === filter.value))
const selected = computed(() => props.items.filter((i) => i.enabled))
const completedCount = computed(() => selected.value.filter(i => i.status === 'success').length)
const iconName = (status: TestItem['status']) => status === 'success' ? 'check' : status === 'running' ? 'play' : status === 'failed' ? 'info' : 'clock'
const statusText = (status: TestItem['status']) => ({ waiting: t('st.waiting'), running: t('st.running'), success: t('st.success'), failed: t('st.failed'), stopped: t('st.stopped') })[status]
const filterLabel = (item: 'ALL' | 'INFO' | 'WARN' | 'ERROR') => ({ ALL: t('st.filterAll'), INFO: t('st.filterInfo'), WARN: t('st.filterWarn'), ERROR: t('st.filterError') })[item]
/** 测试结束后，有历史数据的项目可点击查看其历史曲线 */
const canView = (item: TestItem) => !props.running && (props.history?.[item.id]?.points.length ?? 0) > 0
// 日志新增或整体替换（初始化应答合并）后都滚动到底部：仅盯 length 在「同长度
// 替换」时会漏——历史上一度表现为日志「停在某一段」不跟随最新内容
watch(() => [props.logs.length, props.logs.at(-1)?.time ?? ''], async () => { await nextTick(); if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight })
</script>

<template>
  <aside class="panel status-panel">
    <h2 class="panel-title">{{ mode === 'logs' ? t('st.logsOnly') : t('st.title') }}</h2>
    <template v-if="showQueue">
    <section class="queue-section">
      <div class="section-title"><h3>⌄ {{ t('st.queue') }}</h3><span>{{ completedCount }}/{{ selected.length }}</span></div>
      <div v-if="serverRunning" class="server-badge"><Icon name="monitor" />{{ t('st.serverRunning') }}</div>
      <div class="task-row" v-for="(item, index) in selected" :key="item.id" :class="[item.status, { clickable: canView(item), selected: historySelected === item.id }]" :title="canView(item) ? t('st.viewHistory') : undefined" @click="canView(item) && emit('select', item.id)"><Icon :name="iconName(item.status)" /><span>{{ index + 1 }}. {{ itemLabel(item.id) }}</span><b>{{ statusText(item.status) }}</b></div>
      <div v-if="!selected.length" class="empty">{{ t('st.emptyQueue') }}</div>
    </section>
    </template>
    <section class="logs-section">
      <div class="section-title"><h3>{{ t('st.logsOnly') }}</h3><div class="filters"><button v-for="item in ['ALL','INFO','WARN','ERROR'] as const" :key="item" :class="{ active: filter === item }" @click="filter = item">{{ filterLabel(item) }}</button><button @click="emit('open-log-dir')"><Icon name="folder" :size="13" />{{ t('st.openDir') }}</button><button @click="emit('clear')"><Icon name="trash" :size="13" />{{ t('st.clear') }}</button></div></div>
      <div ref="logBox" class="log-box">
        <p v-for="(log, index) in visibleLogs" :key="index" :class="log.level.toLowerCase()"><span>[{{ log.time }}]</span> <b>[{{ log.level }}]</b> {{ log.message }}</p>
        <div v-if="!visibleLogs.length" class="empty">{{ t('st.emptyLogs') }}</div>
      </div>
    </section>
    <template v-if="showQueue">
    <section class="report-action">
      <button class="success" @click="emit('report')"><Icon name="report" />{{ t('st.report') }}</button>
      <p><Icon name="report" />{{ t('st.reportHint') }}</p>
    </section>
    </template>
  </aside>
</template>
