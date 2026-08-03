<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import TopBar from './components/TopBar.vue'
import ConfigPanel from './components/ConfigPanel.vue'
import Dashboard from './components/Dashboard.vue'
import StatusPanel from './components/StatusPanel.vue'
import ReportSummary from './components/ReportSummary.vue'
import type { BackendEvent, InterfaceInfo, IperfRuntimeInfo, LogEntry, MetricPoint, NetworkInfo, TestConfig, TestItem, TestSummary } from './types'

const defaults: TestConfig = { mode: 'client', protocol: 'tcp', serverIp: '', port: 5201, duration: 30, parallel: 4, bandwidth: -1, packetLength: 1024, interval: 1, iperfPath: 'bundled' }
const config = ref<TestConfig>({ ...defaults })
const items = ref<TestItem[]>([
  { id: 'ping', label: 'Ping 连通性测试', protocol: 'ping', enabled: true, status: 'waiting' },
  { id: 'tcp-single', label: 'TCP 单向带宽', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'tcp-bidir', label: 'TCP 双向带宽', protocol: 'tcp', enabled: false, status: 'waiting' },
  { id: 'tcp-parallel', label: 'TCP 多并发流', protocol: 'tcp', enabled: false, status: 'waiting' },
  { id: 'udp-bandwidth', label: 'UDP 带宽', protocol: 'udp', enabled: false, status: 'waiting' },
  { id: 'udp-loss', label: 'UDP 抖动 / 丢包', protocol: 'udp', enabled: false, status: 'waiting' },
  { id: 'tcp-reverse', label: '反向测试（Reverse）', protocol: 'tcp', enabled: false, status: 'waiting' },
  { id: 'stress', label: '持续时间压力测试', protocol: 'tcp', enabled: false, status: 'waiting' }
])
const local = ref<NetworkInfo>({ ip: '127.0.0.1', mac: '--', hostname: 'localhost', interfaceName: '默认网卡', speedMbps: 0 })
const runtime = ref<IperfRuntimeInfo>({ available: false, bundled: false, path: '', version: '检测中…' })
const logs = ref<LogEntry[]>([])
const points = ref<MetricPoint[]>([])
const running = ref(false)
const activeSession = ref('')
const queue = ref<string[]>([])
const queueIndex = ref(-1)
const progress = ref(0)
const elapsed = ref(0)
const connected = ref(false)
const errorDialog = ref<{ title: string; message: string } | null>(null)
const infoDialog = ref<{ title: string; message: string } | null>(null)
interface RecoveryState { config: TestConfig; queue: string[]; nextIndex: number }
const recovery = ref<RecoveryState | null>(null)
const interfaces = ref<InterfaceInfo[]>([])
const savedCustomLength = ref(0)
const nicDialog = ref(false)
const nicSelected = ref(0)
/** 服务端 IP 尚未被用户手动修改时，重选网卡会同步更新它 */
const autoServerIp = ref(false)
let ticker: number | undefined
let unlisten: UnlistenFn | undefined

const current = computed(() => items.value.find((i) => i.status === 'running'))
const summary = reactive<TestSummary>({ startedAt: '', completed: 0, total: 0, averageBandwidth: 0, maxBandwidth: 0, minBandwidth: 0, totalTransferMb: 0, pingAverage: 0, lossPercent: 0, jitterMs: 0, logPaths: [] })
const isTauri = () => '__TAURI_INTERNALS__' in window
const now = () => new Date().toLocaleTimeString('zh-CN', { hour12: false })
const log = (level: LogEntry['level'], message: string, module = 'UI') => logs.value.push({ time: now(), level, module, message })

watch(points, (value) => {
  const valid = value.filter((p) => p.bandwidthMbps > 0)
  summary.averageBandwidth = valid.length ? valid.reduce((a, b) => a + b.bandwidthMbps, 0) / valid.length : 0
  summary.maxBandwidth = valid.length ? Math.max(...valid.map((p) => p.bandwidthMbps)) : 0
  summary.minBandwidth = valid.length ? Math.min(...valid.map((p) => p.bandwidthMbps)) : 0
  summary.totalTransferMb = value.reduce((a, b) => a + b.transferMb, 0)
  const last = value.at(-1); if (last) { summary.lossPercent = last.lossPercent; summary.jitterMs = last.jitterMs }
}, { deep: true })

watch(() => config.value.serverIp, (value) => { if (value && value !== local.value.ip) autoServerIp.value = false })

/** 应用指定序号的网卡作为本机信息（默认列表第一个） */
function applyNic(index: number) {
  const nic = interfaces.value[index]
  if (!nic) return
  local.value = { ...local.value, ip: nic.ip, mac: nic.mac, interfaceName: nic.interfaceName, speedMbps: nic.speedMbps }
  if (autoServerIp.value) config.value.serverIp = nic.ip
  log('INFO', `已选择网卡：${nic.interfaceName} (${nic.ip})`)
}
function openNicDialog() {
  if (!interfaces.value.length || running.value) return
  nicSelected.value = Math.max(0, interfaces.value.findIndex((i) => i.ip === local.value.ip))
  nicDialog.value = true
}
async function saveCustomLength(length: number) {
  if (!isTauri()) { savedCustomLength.value = length; config.value.packetLength = length; log('INFO', `自定义报文长度 ${length} bytes 已保存（预览模式）`); return }
  try {
    await invoke('save_custom_packet_length', { length })
    savedCustomLength.value = length
    config.value.packetLength = length
    log('INFO', `自定义报文长度 ${length} bytes 已保存到配置文件`)
  } catch (error) { errorDialog.value = { title: '保存失败', message: String(error) } }
}

function toggleItem(id: string) { const item = items.value.find((i) => i.id === id); if (item) item.enabled = !item.enabled }
function reset() { config.value = { ...defaults, mode: config.value.mode, protocol: config.value.protocol, serverIp: local.value.ip, bandwidth: local.value.speedMbps > 0 ? local.value.speedMbps : 0 }; log('INFO', '参数已恢复默认值') }
function clearLogs() { logs.value = [] }
function validate() {
  if (config.value.mode === 'client' && !/^([a-z\d-]+\.)*[a-z\d-]+$/i.test(config.value.serverIp) && !/^\d{1,3}(\.\d{1,3}){3}$/.test(config.value.serverIp)) return '请输入有效的服务端 IP 或主机名'
  if (config.value.port < 1 || config.value.port > 65535) return '端口应在 1–65535 之间'
  if (config.value.duration < 1) return '测试时间必须大于 0 秒'
  return ''
}

async function start() {
  const savedRecovery = recovery.value
  if (savedRecovery) config.value = { ...savedRecovery.config }
  if (config.value.bandwidth < 0) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
  const invalid = validate(); if (invalid) { errorDialog.value = { title: '参数错误', message: invalid }; return }
  const recoveredQueue = savedRecovery?.queue.slice(savedRecovery.nextIndex)
  if (recoveredQueue) items.value.forEach((item) => { item.enabled = recoveredQueue.includes(item.id) })
  const selected = config.value.mode === 'server' ? [] : items.value.filter((i) => i.enabled && (i.protocol === 'ping' || i.protocol === config.value.protocol))
  if (config.value.mode === 'client' && !selected.length) { errorDialog.value = { title: '无法开始', message: '请至少选择一个测试项目。' }; return }
  items.value.forEach((i) => { if (i.enabled) i.status = 'waiting' })
  points.value = []; progress.value = 0; elapsed.value = 0; connected.value = false
  summary.startedAt = new Date().toLocaleString('zh-CN', { hour12: false }); summary.completed = 0; summary.total = config.value.mode === 'server' ? 1 : selected.length; summary.logPaths = []
  queue.value = recoveredQueue || (config.value.mode === 'server' ? ['server'] : selected.map((i) => i.id)); queueIndex.value = -1; running.value = true
  recovery.value = { config: { ...config.value }, queue: [...queue.value], nextIndex: 0 }
  localStorage.setItem('iperf3-gui-recovery', JSON.stringify(recovery.value))
  ticker = window.setInterval(() => { elapsed.value += 1 }, 1000)
  await runNext()
}

async function runNext() {
  queueIndex.value += 1
  if (queueIndex.value >= queue.value.length) { finishRun(true); return }
  const taskId = queue.value[queueIndex.value]
  const item = items.value.find((i) => i.id === taskId); if (item) item.status = 'running'
  progress.value = Math.round((queueIndex.value / Math.max(1, queue.value.length)) * 100)
  log('INFO', `开始${item?.label || 'iperf3 服务端'}`)
  if (!isTauri()) { simulateTask(taskId); return }
  try {
    activeSession.value = await invoke<string>('start_test', { request: { taskId, localIp: local.value.ip, ...config.value } })
  } catch (error) {
    failCurrent(String(error))
  }
}

function simulateTask(taskId: string) {
  let n = 0
  const timer = window.setInterval(() => {
    if (!running.value) { clearInterval(timer); return }
    n++
    if (taskId !== 'ping') points.value.push({ second: n - config.value.duration, bandwidthMbps: 580 + Math.random() * 140, transferMb: 70 + Math.random() * 15, jitterMs: .2, lossPercent: 0, retransmits: 0 })
    if (n >= (taskId === 'ping' ? 3 : Math.min(config.value.duration, 8))) { clearInterval(timer); completeCurrent('success') }
  }, 500)
}

function handleEvent(event: BackendEvent) {
  if (!running.value) return
  if (activeSession.value && event.sessionId !== activeSession.value) return
  if (event.type === 'log') log(event.level || 'INFO', event.message || '', event.taskId)
  if (event.type === 'metric' && event.metric) { points.value.push(event.metric); connected.value = true; if (event.taskId === 'ping' && event.metric.jitterMs) summary.pingAverage = event.metric.jitterMs }
  if (event.logPath && !summary.logPaths.includes(event.logPath)) summary.logPaths.push(event.logPath)
  if (event.type === 'complete') completeCurrent(event.status || 'success')
  if (event.type === 'error') failCurrent(event.message || '测试执行失败')
}

function completeCurrent(status: TestItem['status']) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = status
  if (status === 'success') summary.completed++
  if (recovery.value) { recovery.value.nextIndex = queueIndex.value + 1; localStorage.setItem('iperf3-gui-recovery', JSON.stringify(recovery.value)) }
  progress.value = Math.round(((queueIndex.value + 1) / queue.value.length) * 100)
  if (status === 'failed') { failCurrent('测试进程异常退出'); return }
  void runNext()
}

function failCurrent(message: string) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = 'failed'
  log('ERROR', message); finishRun(false); errorDialog.value = { title: '错误告警', message }
}
function finishRun(completed: boolean) { running.value = false; activeSession.value = ''; connected.value = false; if (ticker) clearInterval(ticker); ticker = undefined; if (completed) { progress.value = 100; recovery.value = null; localStorage.removeItem('iperf3-gui-recovery') } log('INFO', '测试流程结束，日志已保存') }
async function stop() { if (!running.value) return; try { if (isTauri() && activeSession.value) await invoke('stop_test', { sessionId: activeSession.value }) } catch (e) { log('WARN', String(e)) }; const item = current.value; if (item) item.status = 'stopped'; if (recovery.value) { recovery.value.nextIndex = Math.max(0, queueIndex.value); localStorage.setItem('iperf3-gui-recovery', JSON.stringify(recovery.value)) } finishRun(false) }

async function generateReport(format: 'html' | 'pdf' = 'html') {
  try {
    if (isTauri()) {
      const path = await invoke<string>('generate_report', { request: { format, config: config.value, summary: { ...summary }, points: points.value, logs: logs.value } })
      infoDialog.value = { title: '报告已生成', message: path }
    } else infoDialog.value = { title: '预览模式', message: `桌面应用中将生成 ${format.toUpperCase()} 报告。` }
  } catch (error) { errorDialog.value = { title: '报告生成失败', message: String(error) } }
}
function exportConfig() { const a = document.createElement('a'); a.href = URL.createObjectURL(new Blob([JSON.stringify(config.value, null, 2)], { type: 'application/json' })); a.download = 'iperf3-gui-config.json'; a.click(); URL.revokeObjectURL(a.href); log('INFO', '配置已导出') }
function importConfig() { const input = document.createElement('input'); input.type = 'file'; input.accept = '.json'; input.onchange = async () => { const file = input.files?.[0]; if (!file) return; try { config.value = { ...defaults, ...JSON.parse(await file.text()) }; log('INFO', '配置导入成功') } catch { errorDialog.value = { title: '导入失败', message: '配置文件不是有效的 JSON。' } } }; input.click() }
function saveConfig() { localStorage.setItem('iperf3-gui-config', JSON.stringify(config.value)); log('INFO', '配置已保存到本机') }

onMounted(async () => {
  const saved = localStorage.getItem('iperf3-gui-config'); if (saved) try { config.value = { ...defaults, ...JSON.parse(saved) } } catch { /* ignore */ }
  const unfinished = localStorage.getItem('iperf3-gui-recovery'); if (unfinished) try { recovery.value = JSON.parse(unfinished); config.value = { ...defaults, ...recovery.value!.config }; const remaining = recovery.value!.queue.slice(recovery.value!.nextIndex); items.value.forEach((item) => { item.enabled = remaining.includes(item.id); item.status = 'waiting' }); infoDialog.value = { title: '发现未完成测试', message: '上次测试未正常完成。点击“恢复测试”可从中断项目继续。' } } catch { localStorage.removeItem('iperf3-gui-recovery') }
  if (isTauri()) {
    try {
      const [info, ifaces, customLen, runtimeInfo] = await Promise.all([
        invoke<NetworkInfo>('get_network_info'),
        invoke<InterfaceInfo[]>('get_network_interfaces'),
        invoke<number>('get_custom_packet_length'),
        invoke<IperfRuntimeInfo>('get_iperf_runtime_info'),
      ])
      local.value = info
      runtime.value = runtimeInfo
      savedCustomLength.value = customLen
      interfaces.value = ifaces.length ? ifaces : [{ ip: info.ip, mac: info.mac, interfaceName: info.interfaceName, speedMbps: info.speedMbps }]
      // 服务端 IP 为空（或仍是旧版本内置默认值）时视为未手动设置，跟随所选网卡
      autoServerIp.value = !config.value.serverIp || config.value.serverIp === '192.168.1.100'
      applyNic(0)
      // 带宽限制默认取当前网卡最大带宽（-1 表示用户尚未设置过）
      if (config.value.bandwidth === -1) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
      if (interfaces.value.length > 1) { nicSelected.value = 0; nicDialog.value = true }
      unlisten = await listen<BackendEvent>('test-event', (e) => handleEvent(e.payload))
      log(runtime.value.available ? 'INFO' : 'WARN', runtime.value.available ? `${runtime.value.version} 已就绪（${runtime.value.bundled ? '内置' : '系统'}）` : '未找到可用的 iperf3 运行时')
    } catch (e) { log('WARN', `系统信息读取失败：${e}`) }
  } else {
    // 浏览器预览模式：无网卡信息，使用回退默认值
    if (!config.value.serverIp) config.value.serverIp = '127.0.0.1'
    if (config.value.bandwidth === -1) config.value.bandwidth = 100
  }
  log('INFO', 'iperf3 GUI 已就绪')
})
onUnmounted(() => { unlisten?.(); if (ticker) clearInterval(ticker) })
</script>

<template>
  <div class="app-shell">
    <div class="titlebar"><div class="brand-icon">⌁</div><h1>iperf3 GUI Test Tool</h1></div>
    <TopBar :mode="config.mode" :protocol="config.protocol" @update:mode="config.mode = $event" @update:protocol="config.protocol = $event" @import="importConfig" @export="exportConfig" @save="saveConfig" @settings="infoDialog = { title: '运行时设置', message: `${runtime.version}\n来源：${runtime.bundled ? '软件内置' : '系统 PATH / 自定义路径'}\n路径：${runtime.path}` }" @about="infoDialog = { title: '关于', message: 'iperf3 GUI v0.1.0\nRust + Tauri + Vue 3\n内置 iperf3 遵循 BSD-3-Clause 许可证' }" />
    <div class="workspace"><ConfigPanel :config="config" :items="items" :running="running" :recovery="!!recovery" :runtime="runtime" :local="local" :saved-custom-length="savedCustomLength" @update:config="config = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="stop" @report="generateReport('html')" @clear="clearLogs" @pick-nic="openNicDialog" @save-custom-length="saveCustomLength" /><Dashboard :local="local" :config="config" :current="current" :points="points" :progress="progress" :elapsed="elapsed" :summary="summary" :connected="connected" /><StatusPanel :items="items" :logs="logs" :progress="progress" :elapsed="elapsed" :duration="config.duration" @clear="clearLogs" /></div>
    <ReportSummary :config="config" :summary="summary" @report="generateReport" />
    <div v-if="errorDialog || infoDialog" class="modal-backdrop" @click.self="errorDialog = null; infoDialog = null"><div class="modal"><button class="modal-close" @click="errorDialog = null; infoDialog = null">×</button><h2>{{ (errorDialog || infoDialog)?.title }}</h2><div class="modal-body"><span :class="['modal-symbol', errorDialog ? 'error' : 'info']">{{ errorDialog ? '×' : 'i' }}</span><p>{{ (errorDialog || infoDialog)?.message }}</p></div><div class="modal-actions"><button v-if="errorDialog" class="primary" @click="errorDialog = null; start()">重试</button><button @click="errorDialog = null; infoDialog = null">确定</button></div></div></div>
    <div v-if="nicDialog" class="modal-backdrop" @click.self="nicDialog = false; applyNic(0)"><div class="modal nic-modal"><button class="modal-close" @click="nicDialog = false; applyNic(0)">×</button><h2>选择本机网卡</h2><p class="nic-hint">检测到多个网卡，请选择测试使用的本机网卡：</p><div class="nic-list"><label v-for="(nic, index) in interfaces" :key="nic.ip" class="nic-option"><input type="radio" :value="index" v-model="nicSelected" /><span class="nic-name">{{ nic.interfaceName }}</span><span class="nic-ip">{{ nic.ip }}</span><span class="nic-speed">{{ nic.speedMbps ? nic.speedMbps + ' Mbps' : '速率未知' }}</span></label></div><div class="modal-actions"><button class="primary" @click="nicDialog = false; applyNic(nicSelected)">确定</button><button @click="nicDialog = false; applyNic(0)">取消（默认第一个）</button></div></div></div>
  </div>
</template>
