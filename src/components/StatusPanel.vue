<script setup lang="ts">
import { computed, nextTick, ref, watch } from 'vue'
import type { LogEntry, TestItem } from '../types'
import Icon from './Icon.vue'

const props = defineProps<{ items: TestItem[]; logs: LogEntry[]; progress: number; elapsed: number; duration: number }>()
const emit = defineEmits<{ clear: [] }>()
const filter = ref<'ALL' | 'INFO' | 'WARN' | 'ERROR'>('ALL')
const logBox = ref<HTMLElement>()
const visibleLogs = computed(() => filter.value === 'ALL' ? props.logs : props.logs.filter((l) => l.level === filter.value))
const selected = computed(() => props.items.filter((i) => i.enabled))
const iconName = (status: TestItem['status']) => status === 'success' ? 'check' : status === 'running' ? 'play' : status === 'failed' ? 'info' : 'clock'
const statusText = (status: TestItem['status']) => ({ waiting: '等待中', running: '进行中', success: '已完成', failed: '失败', stopped: '已停止' })[status]
const formatTime = (s: number) => `00:${String(Math.floor(s / 60)).padStart(2, '0')}:${String(s % 60).padStart(2, '0')}`
watch(() => props.logs.length, async () => { await nextTick(); if (logBox.value) logBox.value.scrollTop = logBox.value.scrollHeight })
</script>

<template>
  <aside class="panel status-panel">
    <h2 class="panel-title">执行状态 / 日志 / 进度</h2>
    <section class="queue-section">
      <div class="section-title"><h3>⌄ 执行队列</h3><span>{{ selected.filter(i => i.status === 'success').length }}/{{ selected.length }}</span></div>
      <div class="task-row" v-for="(item, index) in selected" :key="item.id" :class="item.status"><Icon :name="iconName(item.status)" /><span>{{ index + 1 }}. {{ item.label }}</span><b>{{ statusText(item.status) }}</b></div>
      <div v-if="!selected.length" class="empty">尚未选择测试项目</div>
    </section>
    <section class="logs-section">
      <div class="section-title"><h3>日志输出</h3><div class="filters"><button v-for="item in ['ALL','INFO','WARN','ERROR'] as const" :key="item" :class="{ active: filter === item }" @click="filter = item">{{ {ALL:'全部',INFO:'信息',WARN:'警告',ERROR:'错误'}[item] }}</button><button @click="emit('clear')"><Icon name="trash" :size="13" />清空</button></div></div>
      <div ref="logBox" class="log-box">
        <p v-for="(log, index) in visibleLogs" :key="index" :class="log.level.toLowerCase()"><span>[{{ log.time }}]</span> <b>[{{ log.level }}]</b> {{ log.message }}</p>
        <div v-if="!visibleLogs.length" class="empty">运行日志将在这里实时显示</div>
      </div>
    </section>
    <section class="side-progress"><div class="section-title"><h3>进度</h3><b>{{ progress }}%</b></div><div class="progress"><i :style="{ width: `${progress}%` }"></i></div><p>已用时：{{ formatTime(elapsed) }} / 预计剩余：{{ formatTime(Math.max(0, duration - elapsed)) }}</p></section>
    <section class="report-state"><h3>报告状态</h3><p><Icon name="report" />已完成项目可生成阶段性报告；中途停止也会生成已完成测试报告。</p></section>
  </aside>
</template>
