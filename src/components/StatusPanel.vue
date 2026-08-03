<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { LogEntry, TestItem } from '../types'
import Icon from './Icon.vue'

const props = defineProps<{ items: TestItem[]; logs: LogEntry[]; serverRunning: boolean; mode?: 'full' | 'logs' }>()
const emit = defineEmits<{ clear: []; 'open-log-dir': []; report: [] }>()
/** logs 模式（服务端分离窗口）只显示日志，隐藏客户端执行队列与报告区 */
const showQueue = computed(() => props.mode !== 'logs')
const filter = ref<'ALL' | 'INFO' | 'WARN' | 'ERROR'>('ALL')
const logBox = ref<HTMLElement>()
const visibleLogs = computed(() => filter.value === 'ALL' ? props.logs : props.logs.filter((l) => l.level === filter.value))
const selected = computed(() => props.items.filter((i) => i.enabled))
const iconName = (status: TestItem['status']) => status === 'success' ? 'check' : status === 'running' ? 'play' : status === 'failed' ? 'info' : 'clock'
const statusText = (status: TestItem['status']) => ({ waiting: '等待中', running: '进行中', success: '已完成', failed: '失败', stopped: '已停止' })[status]
watch(() => props.logs.length, async () => { await nextTick(); if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight })
</script>

<template>
  <aside class="panel status-panel">
    <h2 class="panel-title">{{ mode === 'logs' ? '日志输出' : '执行状态 / 日志' }}</h2>
    <template v-if="showQueue">
    <section class="queue-section">
      <div class="section-title"><h3>⌄ 执行队列</h3><span>{{ selected.filter(i => i.status === 'success').length }}/{{ selected.length }}</span></div>
      <div v-if="serverRunning" class="server-badge"><Icon name="monitor" />riperf3 服务端运行中</div>
      <div class="task-row" v-for="(item, index) in selected" :key="item.id" :class="item.status"><Icon :name="iconName(item.status)" /><span>{{ index + 1 }}. {{ item.label }}</span><b>{{ statusText(item.status) }}</b></div>
      <div v-if="!selected.length" class="empty">尚未选择测试项目</div>
    </section>
    </template>
    <section class="logs-section">
      <div class="section-title"><h3>日志输出</h3><div class="filters"><button v-for="item in ['ALL','INFO','WARN','ERROR'] as const" :key="item" :class="{ active: filter === item }" @click="filter = item">{{ {ALL:'全部',INFO:'信息',WARN:'警告',ERROR:'错误'}[item] }}</button><button @click="emit('open-log-dir')"><Icon name="folder" :size="13" />打开目录</button><button @click="emit('clear')"><Icon name="trash" :size="13" />清空</button></div></div>
      <div ref="logBox" class="log-box">
        <p v-for="(log, index) in visibleLogs" :key="index" :class="log.level.toLowerCase()"><span>[{{ log.time }}]</span> <b>[{{ log.level }}]</b> {{ log.message }}</p>
        <div v-if="!visibleLogs.length" class="empty">运行日志将在这里实时显示</div>
      </div>
    </section>
    <template v-if="showQueue">
    <section class="report-action">
      <button class="success" @click="emit('report')"><Icon name="report" />生成报告</button>
      <p><Icon name="report" />已完成项目可生成阶段性报告；中途停止也会生成已完成测试报告。</p>
    </section>
    </template>
  </aside>
</template>
