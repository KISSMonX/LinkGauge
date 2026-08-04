<script setup lang="ts">
/** SSH 远程控制台：在远端主机上操作 iperf3 服务端，实时显示输出。
 *  行缓冲由 App.vue 持有（切换到概览标签时不丢内容），这里只负责渲染与输入。 */
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from 'vue'
import type { SshStatus } from '../types'
import { useI18n } from '../i18n'
import Icon from './Icon.vue'
import ServerViewTabs from './ServerViewTabs.vue'

const props = defineProps<{
  view: 'overview' | 'ssh'
  lines: string[]
  status: SshStatus
  host: string
  username: string
  /** 服务端标签页配置的监听端口与统计间隔：快捷命令按它拼 iperf3 参数 */
  serverPort: number
  interval: number
}>()
const emit = defineEmits<{
  'update:view': [value: 'overview' | 'ssh']
  /** 向远端 shell 写入原始数据（命令需自带换行） */
  send: [data: string]
  clear: []
  connect: []
  disconnect: []
  /** 控制台可视尺寸变化 → 同步远端 PTY 窗口大小 */
  resize: [cols: number, rows: number]
}>()

const { t } = useI18n()
const connected = computed(() => props.status === 'connected')
const text = computed(() => props.lines.join('\n'))

/** 远端 iperf3 服务端的常用操作：按服务端标签页的端口 / 间隔拼命令 */
const quickCommands = computed(() => [
  { key: 'start', label: t('ssh.cmd.start'), command: `iperf3 -s -p ${props.serverPort} -i ${props.interval}` },
  { key: 'daemon', label: t('ssh.cmd.daemon'), command: `iperf3 -s -p ${props.serverPort} -i ${props.interval} -D` },
  { key: 'status', label: t('ssh.cmd.status'), command: 'ps -ef | grep -w "[i]perf3"' },
  { key: 'port', label: t('ssh.cmd.port'), command: `ss -lntup 2>/dev/null | grep ":${props.serverPort}" || netstat -lntup | grep ":${props.serverPort}"` },
  { key: 'kill', label: t('ssh.cmd.kill'), command: 'pkill -f "iperf3 -s"' },
  { key: 'version', label: t('ssh.cmd.version'), command: 'iperf3 --version' },
])

// —— 命令输入：回车执行，↑ / ↓ 翻阅历史 ——
const command = ref('')
const history = ref<string[]>([])
const historyIndex = ref(-1)

function run(line: string) {
  if (!connected.value) return
  emit('send', `${line}\n`)
  if (line.trim() && history.value.at(-1) !== line) history.value.push(line)
  historyIndex.value = -1
}
function submit() {
  const line = command.value
  command.value = ''
  run(line)
}
function browseHistory(step: number) {
  if (!history.value.length) return
  const next = historyIndex.value < 0 ? history.value.length + step : historyIndex.value + step
  if (next >= history.value.length || next < 0) { historyIndex.value = -1; command.value = ''; return }
  historyIndex.value = next
  command.value = history.value[next]
}
/** Ctrl+C：中断远端正在前台运行的 iperf3 */
function interrupt() { if (connected.value) emit('send', '\x03') }

// —— 输出区：跟随滚动到底部（用户向上翻阅时暂停跟随）——
const box = ref<HTMLElement>()
const stick = ref(true)
function onScroll() {
  const el = box.value
  if (!el) return
  stick.value = el.scrollHeight - el.scrollTop - el.clientHeight < 24
}
watch(text, async () => {
  if (!stick.value) return
  await nextTick()
  if (box.value) box.value.scrollTop = box.value.scrollHeight
})
watch(() => props.view, async (view) => {
  if (view !== 'ssh') return
  await nextTick()
  measure()
  if (stick.value && box.value) box.value.scrollTop = box.value.scrollHeight
})

// —— 远端 PTY 尺寸：按输出区实际像素与字符尺寸换算（ruler 是等宽的 10 个 0）——
const ruler = ref<HTMLElement>()
function measure() {
  const el = box.value
  const cell = ruler.value
  if (!el || !cell || !el.clientWidth) return
  const size = cell.getBoundingClientRect()
  const charWidth = size.width / 10 || 8
  const lineHeight = size.height || 16
  const cols = Math.max(40, Math.floor((el.clientWidth - 16) / charWidth))
  const rows = Math.max(10, Math.floor((el.clientHeight - 12) / lineHeight))
  emit('resize', cols, rows)
}
let observer: ResizeObserver | undefined
onMounted(() => {
  measure()
  if (box.value && 'ResizeObserver' in window) {
    observer = new ResizeObserver(() => measure())
    observer.observe(box.value)
  }
})
onUnmounted(() => observer?.disconnect())
</script>

<template>
  <main class="panel dashboard ssh-console">
    <div class="panel-title console-title">
      <ServerViewTabs :view="view" :ssh-status="status" @update:view="emit('update:view', $event)" />
      <span class="console-peer">{{ connected ? `${username}@${host}` : t('ssh.notConnected') }}</span>
    </div>
    <div class="console-toolbar">
      <div class="console-quick">
        <button v-for="item in quickCommands" :key="item.key" :disabled="!connected" :title="item.command" @click="run(item.command)">{{ item.label }}</button>
        <button class="warn" :disabled="!connected" :title="t('ssh.interruptHint')" @click="interrupt">^C</button>
      </div>
      <div class="console-actions">
        <button v-if="!connected" class="primary" :disabled="status === 'connecting'" @click="emit('connect')"><Icon name="link" :size="13" />{{ status === 'connecting' ? t('ssh.connecting') : t('ssh.connect') }}</button>
        <button v-else class="danger" @click="emit('disconnect')"><Icon name="unlink" :size="13" />{{ t('ssh.disconnect') }}</button>
        <button @click="emit('clear')"><Icon name="trash" :size="13" />{{ t('st.clear') }}</button>
      </div>
    </div>
    <pre ref="box" class="console-output" @scroll="onScroll">{{ text }}<span v-if="!lines.some(Boolean)" class="console-empty">{{ t('ssh.empty') }}</span></pre>
    <span ref="ruler" class="console-ruler" aria-hidden="true">0000000000</span>
    <form class="console-input" @submit.prevent="submit">
      <span class="console-prompt">$</span>
      <input
        v-model="command" :disabled="!connected" :placeholder="connected ? t('ssh.inputPlaceholder') : t('ssh.inputDisabled')"
        autocomplete="off" spellcheck="false"
        @keydown.up.prevent="browseHistory(-1)" @keydown.down.prevent="browseHistory(1)"
      />
      <button type="submit" class="primary" :disabled="!connected">{{ t('ssh.send') }}</button>
    </form>
  </main>
</template>
