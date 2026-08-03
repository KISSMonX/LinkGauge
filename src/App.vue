<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { save } from '@tauri-apps/plugin-dialog'
import ConfigPanel from './components/ConfigPanel.vue'
import Dashboard from './components/Dashboard.vue'
import StatusPanel from './components/StatusPanel.vue'
import ReportSummary from './components/ReportSummary.vue'
import Icon from './components/Icon.vue'
import type { BackendEvent, DockEvent, InterfaceInfo, LogEntry, MetricPoint, NetworkInfo, ServerConfig, SyncState, TestConfig, TestItem, TestSummary } from './types'

const defaults: TestConfig = { mode: 'client', serverIp: '', port: 5201, duration: 30, parallel: 4, bandwidth: -1, packetLength: 131072, udpPacketLength: 8192, interval: 1 }
const serverDefaults: ServerConfig = { port: 5201 }
const config = ref<TestConfig>({ ...defaults })
const serverConfig = ref<ServerConfig>({ ...serverDefaults })
const items = ref<TestItem[]>([
  { id: 'ping', label: 'Ping 连通性测试', protocol: 'ping', enabled: true, status: 'waiting' },
  { id: 'tcp-single', label: 'TCP 单向带宽', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'tcp-bidir', label: 'TCP 双向带宽', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'tcp-parallel', label: 'TCP 多并发流', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'udp-bandwidth', label: 'UDP 带宽', protocol: 'udp', enabled: true, status: 'waiting' },
  { id: 'udp-loss', label: 'UDP 抖动 / 丢包', protocol: 'udp', enabled: true, status: 'waiting' },
  { id: 'tcp-reverse', label: '反向测试（Reverse）', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'stress', label: '持续时间压力测试', protocol: 'tcp', enabled: true, status: 'waiting' }
])
const local = ref<NetworkInfo>({ ip: '127.0.0.1', mac: '--', hostname: 'localhost', interfaceName: '默认网卡', speedMbps: 0 })
const logs = ref<LogEntry[]>([])
const points = ref<MetricPoint[]>([])
/** 最近一次已完成测试的完整数据缓存（仅内存，退出后销毁），完成后图表显示它 */
const completedPoints = ref<MetricPoint[]>([])
const completedLabel = ref('')
/** 图表显示数据：测试进行中显示实时数据，否则显示最近一次完整测试的数据 */
const chartPoints = computed(() => (current.value ? points.value : completedPoints.value))
const chartLive = computed(() => !!current.value)
const clientRunning = ref(false)
const serverRunning = ref(false)
const clientSession = ref('')
const serverSession = ref('')
const activeTab = ref<'client' | 'server'>('client')
const queue = ref<string[]>([])
const queueIndex = ref(-1)
const progress = ref(0)
const elapsed = ref(0)
const connected = ref(false)
const errorDialog = ref<{ title: string; message: string } | null>(null)
/** recovery 为 true 表示「发现未完成测试」弹窗，提供恢复/放弃两个选项 */
const infoDialog = ref<{ title: string; message: string; recovery?: boolean } | null>(null)
interface RecoveryState { config: TestConfig; queue: string[]; nextIndex: number }
const recovery = ref<RecoveryState | null>(null)
const interfaces = ref<InterfaceInfo[]>([])
const savedTcpLength = ref(0)
const savedUdpLength = ref(0)
const nicDialog = ref(false)
const nicSelected = ref(0)
/** 服务端 IP 尚未被用户手动修改时，重选网卡会同步更新它 */
const autoServerIp = ref(false)
let ticker: number | undefined
let unlisten: UnlistenFn | undefined

// —— 多窗口支持：窗口角色（主窗口 hub / 分离的 client / server）+ 标签停靠状态 ——
const isTauri = () => '__TAURI_INTERNALS__' in window
const ownLabel = isTauri() ? getCurrentWebviewWindow().label : 'main'
/** hub = 主窗口（标签页容器）；client / server = 分离出的独立窗口 */
const side = ref<'hub' | 'client' | 'server'>(ownLabel === 'main' ? 'hub' : ownLabel === 'client' || ownLabel === 'server' ? ownLabel : 'hub')
/** 主窗口中当前停靠的标签页列表（顺序固定为客户端在前） */
const dockedTabs = ref<('client' | 'server')[]>(['client', 'server'])
/** 驱动客户端任务队列的窗口 label：谁点「开始测试」谁驱动，其他窗口只展示不重复启动 */
const driver = ref('')
let syncing = false
let unlistenSync: UnlistenFn | undefined
let unlistenDock: UnlistenFn | undefined
let unlistenClose: UnlistenFn | undefined

const current = computed(() => items.value.find((i) => i.status === 'running'))
const summary = reactive<TestSummary>({ startedAt: '', completed: 0, total: 0, averageBandwidth: 0, maxBandwidth: 0, minBandwidth: 0, totalTransferMb: 0, pingAverage: 0, lossPercent: 0, jitterMs: 0, logPaths: [] })
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
/** 参数自动保存：任何修改即时写入本地存储，下次启动读取最近一次配置 */
watch(config, (value) => { localStorage.setItem('linkgauge-config', JSON.stringify(value)); if (!syncing) emitSync() }, { deep: true })
/** 服务端参数本地持久化（与客户端配置分开保存） */
watch(serverConfig, (value) => { localStorage.setItem('linkgauge-server-config', JSON.stringify(value)); if (!syncing) emitSync() }, { deep: true })

// —— 跨窗口状态同步：任何本地变更广播 side-sync，其他窗口应用（syncing 标志防回声循环） ——
function syncBundle(): SyncState {
  return {
    config: config.value,
    serverConfig: serverConfig.value,
    items: items.value,
    local: local.value,
    clientRunning: clientRunning.value,
    serverRunning: serverRunning.value,
    clientSession: clientSession.value,
    serverSession: serverSession.value,
    queue: queue.value,
    queueIndex: queueIndex.value,
    driver: driver.value,
    recovery: !!recovery.value,
    savedTcpLength: savedTcpLength.value,
    savedUdpLength: savedUdpLength.value,
    summary: { ...summary },
  }
}
function emitSync() {
  if (!isTauri() || syncing) return
  void emit('side-sync', syncBundle())
}
async function applySync(payload: SyncState) {
  syncing = true
  config.value = payload.config
  serverConfig.value = payload.serverConfig
  items.value = payload.items
  local.value = payload.local
  clientRunning.value = payload.clientRunning
  serverRunning.value = payload.serverRunning
  clientSession.value = payload.clientSession
  serverSession.value = payload.serverSession
  queue.value = payload.queue
  queueIndex.value = payload.queueIndex
  driver.value = payload.driver
  recovery.value = payload.recovery ? { config: payload.config, queue: [...payload.queue], nextIndex: payload.queueIndex } : null
  savedTcpLength.value = payload.savedTcpLength
  savedUdpLength.value = payload.savedUdpLength
  Object.assign(summary, payload.summary)
  // 等本次 Vue 变更刷完再解除标志，避免应用远端状态触发的 watcher 再广播回去
  await nextTick()
  syncing = false
}
/** 运行状态/会话等变化即时同步；计时器只在运行期间走动 */
watch(clientRunning, (running) => {
  if (running) { elapsed.value = 0; if (ticker === undefined) ticker = window.setInterval(() => { elapsed.value += 1 }, 1000) }
  else if (ticker !== undefined) { clearInterval(ticker); ticker = undefined }
  if (!syncing) emitSync()
})
watch(serverRunning, () => emitSync())
watch(clientSession, () => emitSync())
watch(serverSession, () => emitSync())
watch(queue, () => emitSync())
watch(queueIndex, () => emitSync())
watch(driver, () => emitSync())
watch(recovery, () => emitSync())
watch(items, () => emitSync(), { deep: true })
watch(local, () => emitSync())
watch(savedTcpLength, () => emitSync())
watch(savedUdpLength, () => emitSync())
watch(summary, () => emitSync(), { deep: true })

// —— 标签页分离 / 收回 ——
/** 主窗口：标签被拖拽分离 → 创建独立窗口并从停靠列表移除 */
function detachTab(s: 'client' | 'server') {
  if (side.value !== 'hub' || !dockedTabs.value.includes(s)) return
  dockedTabs.value = dockedTabs.value.filter((x) => x !== s)
  if (activeTab.value === s) activeTab.value = dockedTabs.value[0] ?? 'client'
  if (!isTauri()) return
  invoke('create_side_window', { side: s }).catch((e) => {
    // 创建失败则把标签放回
    dockedTabs.value = (['client', 'server'] as ('client' | 'server')[]).filter((x) => dockedTabs.value.includes(x) || x === s)
    errorDialog.value = { title: '窗口创建失败', message: String(e) }
  })
  log('INFO', `「${s === 'client' ? '客户端' : '服务端'}」标签已分离为独立窗口，关闭子窗口可收回`)
}
/** 主窗口：收到子窗口关闭通知（side-dock）后把标签加回 */
function dockTab(s: 'client' | 'server') {
  if (side.value !== 'hub') return
  const wasDetached = !dockedTabs.value.includes(s)
  dockedTabs.value = (['client', 'server'] as ('client' | 'server')[]).filter((x) => dockedTabs.value.includes(x) || x === s)
  // 驱动窗口被收回时由主窗口接管队列驱动；若正处在任务间隙则补跑下一项
  if (wasDetached && driver.value === s && clientRunning.value) {
    driver.value = ownLabel
    if (!current.value && queueIndex.value + 1 < queue.value.length) void runNext()
  }
}
/** 主窗口空状态：请求子窗口自行关闭（走 side-close → 正常收回流程），超时兜底直接收回 */
async function requestDock(s: 'client' | 'server') {
  if (side.value !== 'hub') return
  try { await emit('side-close', { side: s }) } catch (e) { log('WARN', String(e)) }
  setTimeout(() => dockTab(s), 600)
}
/** 子窗口：通知主窗口收回标签并销毁自己（通知失败也要销毁，避免窗口残留） */
async function dockBack() {
  try {
    await emit('side-dock', { side: side.value })
  } catch (e) { log('WARN', `收回标签通知失败：${e}`) }
  try {
    await getCurrentWindow().destroy()
  } catch (e) { log('ERROR', `收回标签失败：${e}`) }
}

/** 应用指定序号的网卡作为本机信息（默认列表第一个） */
function applyNic(index: number) {
  const nic = interfaces.value[index]
  if (!nic) return
  // 带宽限制仍是旧网卡默认值时，跟随新网卡速率（用户手动改过则保留）
  const bandwidthIsNicDefault = config.value.bandwidth === local.value.speedMbps
  local.value = { ...local.value, ip: nic.ip, mac: nic.mac, interfaceName: nic.interfaceName, speedMbps: nic.speedMbps }
  if (bandwidthIsNicDefault) config.value.bandwidth = nic.speedMbps > 0 ? nic.speedMbps : 0
  if (autoServerIp.value) config.value.serverIp = nic.ip
  log('INFO', `已选择网卡：${nic.interfaceName} (${nic.ip})`)
}
function openNicDialog() {
  if (!interfaces.value.length || clientRunning.value) return
  nicSelected.value = Math.max(0, interfaces.value.findIndex((i) => i.ip === local.value.ip))
  nicDialog.value = true
}
async function saveCustomLength(protocol: 'tcp' | 'udp', length: number) {
  const isTcp = protocol === 'tcp'
  if (!isTauri()) { if (isTcp) { savedTcpLength.value = length; config.value.packetLength = length } else { savedUdpLength.value = length; config.value.udpPacketLength = length } log('INFO', `自定义${isTcp ? 'TCP' : 'UDP'}报文长度 ${length} bytes 已保存（预览模式）`); return }
  try {
    await invoke('save_custom_packet_length', { protocol, length })
    if (isTcp) { savedTcpLength.value = length; config.value.packetLength = length } else { savedUdpLength.value = length; config.value.udpPacketLength = length }
    log('INFO', `自定义${isTcp ? 'TCP' : 'UDP'}报文长度 ${length} bytes 已保存到配置文件`)
  } catch (error) { errorDialog.value = { title: '保存失败', message: String(error) } }
}

function toggleItem(id: string) { const item = items.value.find((i) => i.id === id); if (item) item.enabled = !item.enabled }
function reset() { config.value = { ...defaults, mode: config.value.mode, serverIp: local.value.ip, bandwidth: local.value.speedMbps > 0 ? local.value.speedMbps : 0 }; log('INFO', '参数已恢复默认值') }
function clearLogs() { logs.value = [] }
function validate() {
  if (config.value.mode === 'client' && !/^([a-z\d-]+\.)*[a-z\d-]+$/i.test(config.value.serverIp) && !/^\d{1,3}(\.\d{1,3}){3}$/.test(config.value.serverIp)) return '请输入有效的服务端 IP 或主机名'
  if (config.value.port < 1 || config.value.port > 65535) return '端口应在 1–65535 之间'
  if (config.value.duration < 1) return '测试时间必须大于 0 秒'
  return ''
}

async function start() {
  const savedRecovery = recovery.value
  // 恢复测试时参数以用户当前界面为准（应用启动时已从恢复状态加载过上次参数），
  // 不再用开始测试时的快照覆盖，避免用户修改的端口等参数被回退
  if (config.value.bandwidth < 0) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
  const invalid = validate(); if (invalid) { errorDialog.value = { title: '参数错误', message: invalid }; return }
  const recoveredQueue = savedRecovery?.queue.slice(savedRecovery.nextIndex)
  if (recoveredQueue) items.value.forEach((item) => { item.enabled = recoveredQueue.includes(item.id) })
  // TCP / UDP 测试项可同时勾选，按列表顺序逐个执行
  const selected = items.value.filter((i) => i.enabled)
  if (!selected.length) { errorDialog.value = { title: '无法开始', message: '请至少选择一个测试项目。' }; return }
  items.value.forEach((i) => { if (i.enabled) i.status = 'waiting' })
  points.value = []; progress.value = 0; elapsed.value = 0; connected.value = false
  summary.startedAt = new Date().toLocaleString('zh-CN', { hour12: false }); summary.completed = 0; summary.total = selected.length; summary.logPaths = []
  queue.value = recoveredQueue || selected.map((i) => i.id); queueIndex.value = -1; clientRunning.value = true
  // 本窗口成为队列驱动者：只有它会在任务结束后启动下一项
  driver.value = ownLabel
  config.value.mode = 'client'
  recovery.value = { config: { ...config.value }, queue: [...queue.value], nextIndex: 0 }
  localStorage.setItem('linkgauge-recovery', JSON.stringify(recovery.value))
  await runNext()
}

/** 启动 riperf3 服务端（独立于客户端任务队列，两者可同时运行） */
async function startServer() {
  if (serverRunning.value) return
  if (serverConfig.value.port < 1 || serverConfig.value.port > 65535) { errorDialog.value = { title: '参数错误', message: '端口应在 1–65535 之间' }; return }
  log('INFO', `正在启动 riperf3 服务端，监听端口 ${serverConfig.value.port}…`)
  if (!isTauri()) { serverRunning.value = true; log('INFO', '预览模式：模拟服务端启动'); return }
  try {
    serverSession.value = await invoke<string>('start_test', { request: { taskId: 'server', mode: 'server', protocol: 'tcp', serverIp: local.value.ip, localIp: local.value.ip, port: serverConfig.value.port, duration: 0, parallel: 0, bandwidth: 0, packetLength: 0, interval: 1 } })
    serverRunning.value = true
  } catch (error) { log('ERROR', String(error)); errorDialog.value = { title: '服务端启动失败', message: String(error) } }
}
async function stopServer() {
  if (!serverRunning.value) return
  try { if (isTauri() && serverSession.value) await invoke('stop_test', { sessionId: serverSession.value }) } catch (e) { log('WARN', String(e)) }
  serverRunning.value = false; serverSession.value = ''
  log('INFO', '已请求停止服务端，端口将释放')
}

async function runNext() {
  queueIndex.value += 1
  if (queueIndex.value >= queue.value.length) { finishRun(true); return }
  const taskId = queue.value[queueIndex.value]
  const item = items.value.find((i) => i.id === taskId); if (item) item.status = 'running'
  progress.value = Math.round((queueIndex.value / Math.max(1, queue.value.length)) * 100)
  // 每个任务独立缓存：开始时清空实时数据，完成后快照到 completedPoints
  points.value = []
  log('INFO', `开始${item?.label || 'riperf3 服务端'}`)
  if (!isTauri()) { simulateTask(taskId); return }
  try {
    clientSession.value = await invoke<string>('start_test', { request: { taskId, localIp: local.value.ip, ...config.value } })
  } catch (error) {
    failCurrent(String(error))
  }
}

function simulateTask(taskId: string) {
  let n = 0
  const timer = window.setInterval(() => {
    if (!clientRunning.value) { clearInterval(timer); return }
    n++
    if (taskId !== 'ping') points.value.push({ second: n - config.value.duration, bandwidthMbps: 580 + Math.random() * 140, transferMb: 70 + Math.random() * 15, jitterMs: .2, lossPercent: 0, retransmits: 0 })
    if (n >= (taskId === 'ping' ? 3 : Math.min(config.value.duration, 8))) { clearInterval(timer); completeCurrent('success') }
  }, 500)
}

function handleEvent(event: BackendEvent) {
  const isClient = event.sessionId === clientSession.value
  const isServer = event.sessionId === serverSession.value
  if (!isClient && !isServer) return
  if (event.type === 'log') log(event.level || 'INFO', event.message || '', event.taskId)
  if (event.logPath && !summary.logPaths.includes(event.logPath)) summary.logPaths.push(event.logPath)
  // 服务端事件：独立于客户端队列，仅维护运行状态
  if (isServer) {
    if (event.type === 'complete') { serverRunning.value = false; serverSession.value = ''; log('INFO', `服务端已停止（${event.status === 'stopped' ? '手动停止' : '已结束'}）`) }
    if (event.type === 'error') { serverRunning.value = false; serverSession.value = ''; log('ERROR', event.message || '服务端异常退出'); errorDialog.value = { title: '服务端错误', message: event.message || '服务端异常退出' } }
    return
  }
  // 客户端事件：停止或结束后的事件忽略
  if (!clientRunning.value) return
  if (event.type === 'metric' && event.metric) { points.value.push(event.metric); connected.value = true; if (event.taskId === 'ping' && event.metric.jitterMs) summary.pingAverage = event.metric.jitterMs }
  if (event.type === 'complete') completeCurrent(event.status || 'success')
  if (event.type === 'error') failCurrent(event.message || '测试执行失败')
}

function completeCurrent(status: TestItem['status']) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = status
  if (status === 'success') summary.completed++
  // 快照本次测试的完整数据到内存缓存（测试完成后图表显示；退出应用即销毁）
  completedPoints.value = [...points.value]
  if (item) completedLabel.value = item.label
  if (recovery.value) { recovery.value.nextIndex = queueIndex.value + 1; localStorage.setItem('linkgauge-recovery', JSON.stringify(recovery.value)) }
  progress.value = Math.round(((queueIndex.value + 1) / queue.value.length) * 100)
  if (status === 'failed') { failCurrent('测试进程异常退出'); return }
  // 仅驱动窗口启动下一项，避免多个窗口重复拉起同一个任务
  if (driver.value === ownLabel) void runNext()
}

function failCurrent(message: string) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = 'failed'
  completedPoints.value = [...points.value]
  if (item) completedLabel.value = item.label
  log('ERROR', message); finishRun(false)
  const lastLog = summary.logPaths.at(-1)
  errorDialog.value = { title: '错误告警', message: lastLog ? `${message}\n\n日志文件：${lastLog}` : message }
}
function finishRun(completed: boolean) { clientRunning.value = false; clientSession.value = ''; connected.value = false; if (completed) { progress.value = 100; recovery.value = null; localStorage.removeItem('linkgauge-recovery') } log('INFO', '测试流程结束，日志已保存') }
async function stop() { if (!clientRunning.value) return; completedPoints.value = [...points.value]; const item = current.value; if (item) completedLabel.value = item.label; try { if (isTauri() && clientSession.value) await invoke('stop_test', { sessionId: clientSession.value }) } catch (e) { log('WARN', String(e)) }; if (item) item.status = 'stopped'; if (recovery.value) { recovery.value.nextIndex = Math.max(0, queueIndex.value); localStorage.setItem('linkgauge-recovery', JSON.stringify(recovery.value)) } finishRun(false) }
/** 放弃未完成的测试队列并恢复默认全选，重新开始新一轮测试 */
function discardRecovery() {
  recovery.value = null
  localStorage.removeItem('linkgauge-recovery')
  items.value.forEach((item) => { item.enabled = true; item.status = 'waiting' })
  log('INFO', '已放弃上次未完成的测试队列，可重新开始')
}

const reportStamp = () => {
  const n = new Date()
  const p = (v: number) => String(v).padStart(2, '0')
  return `${n.getFullYear()}${p(n.getMonth() + 1)}${p(n.getDate())}${p(n.getHours())}${p(n.getMinutes())}${p(n.getSeconds())}`
}
/** 弹出系统保存对话框，让用户选择保存目录并自定义文件名，确认后生成报告 */
async function generateReport(format: 'html' | 'pdf' = 'html') {
  if (!isTauri()) { infoDialog.value = { title: '预览模式', message: `桌面应用中将生成 ${format.toUpperCase()} 报告。` }; return }
  try {
    const dir = await invoke<string>('get_report_dir')
    const path = await save({
      title: '保存测试报告',
      defaultPath: `${dir}\\linkgauge-report-${reportStamp()}.${format}`,
      filters: [{ name: format.toUpperCase(), extensions: [format] }]
    })
    if (!path) return // 用户取消
    const saved = await invoke<string>('generate_report', { request: { format, savePath: path, config: config.value, summary: { ...summary }, points: chartPoints.value, logs: logs.value } })
    infoDialog.value = { title: '报告已生成', message: saved }
  } catch (error) { errorDialog.value = { title: '报告生成失败', message: String(error) } }
}
/** 用系统文件管理器打开报告输出目录 */
async function openReportDir() {
  try {
    const dir = isTauri() ? await invoke<string>('open_report_dir') : '桌面应用中将打开报告输出目录（应用数据目录下 reports/）'
    log('INFO', `报告目录：${dir}`)
  } catch (error) { errorDialog.value = { title: '打开目录失败', message: String(error) } }
}
/** 用系统文件管理器打开测试日志目录 */
async function openLogDir() {
  try {
    const dir = isTauri() ? await invoke<string>('open_log_dir') : '桌面应用中将打开测试日志目录（应用日志目录下 tests/）'
    log('INFO', `日志目录：${dir}`)
  } catch (error) { errorDialog.value = { title: '打开目录失败', message: String(error) } }
}
/** 导出配置：弹出保存对话框，默认程序安装目录 config/config.json */
async function exportConfig() {
  if (!isTauri()) { infoDialog.value = { title: '预览模式', message: '桌面应用中将弹出保存对话框导出配置。' }; return }
  try {
    const dir = await invoke<string>('get_export_dir')
    const path = await save({ title: '导出配置', defaultPath: `${dir}\\config.json`, filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (!path) return // 用户取消
    const saved = await invoke<string>('export_config', { path, config: JSON.stringify(config.value, null, 2) })
    infoDialog.value = { title: '配置已导出', message: saved }
  } catch (error) { errorDialog.value = { title: '导出失败', message: String(error) } }
}
function importConfig() { const input = document.createElement('input'); input.type = 'file'; input.accept = '.json'; input.onchange = async () => { const file = input.files?.[0]; if (!file) return; try { config.value = { ...defaults, ...JSON.parse(await file.text()) }; log('INFO', '配置导入成功') } catch { errorDialog.value = { title: '导入失败', message: '配置文件不是有效的 JSON。' } } }; input.click() }

onMounted(async () => {
  const saved = localStorage.getItem('linkgauge-config'); if (saved) try { const parsed = JSON.parse(saved); if (parsed.packetLength === 1024 || parsed.packetLength === 4096) parsed.packetLength = 131072 /* 旧默认值迁移为新默认 128KB */; config.value = { ...defaults, ...parsed } } catch { /* ignore */ }
  const savedServer = localStorage.getItem('linkgauge-server-config'); if (savedServer) try { serverConfig.value = { ...serverDefaults, ...JSON.parse(savedServer) } } catch { /* ignore */ }
  const unfinished = localStorage.getItem('linkgauge-recovery'); if (unfinished) try { recovery.value = JSON.parse(unfinished); config.value = { ...defaults, ...recovery.value!.config }; const remaining = recovery.value!.queue.slice(recovery.value!.nextIndex); items.value.forEach((item) => { item.enabled = remaining.includes(item.id); item.status = 'waiting' }); /* 恢复确认弹窗只在主窗口弹出，分离窗口静默加载状态（运行中的状态随后由同步事件更新） */ if (side.value === 'hub') infoDialog.value = { title: '发现未完成测试', message: '上次测试未正常完成，是否继续上次的测试队列？', recovery: true } } catch { localStorage.removeItem('linkgauge-recovery') }
  if (isTauri()) {
    try {
      // 跨窗口同步：状态包广播 + 主窗口收回标签通知 + 子窗口被请求关闭
      unlistenSync = await listen<SyncState>('side-sync', (e) => applySync(e.payload))
      if (side.value === 'hub') {
        unlistenDock = await listen<DockEvent>('side-dock', (e) => dockTab(e.payload.side))
      } else {
        // 子窗口关闭（右上角 ✕）= 标签收回主窗口，而不是退出
        unlistenDock = await getCurrentWindow().onCloseRequested(async (event) => {
          event.preventDefault()
          try {
            await emit('side-dock', { side: side.value })
          } catch (e) { log('WARN', `收回标签通知失败：${e}`) }
          await getCurrentWindow().destroy()
        })
        unlistenClose = await listen<DockEvent>('side-close', (e) => { if (e.payload.side === side.value) void dockBack() })
      }
      const [info, ifaces, customTcpLen, customUdpLen] = await Promise.all([
        invoke<NetworkInfo>('get_network_info'),
        invoke<InterfaceInfo[]>('get_network_interfaces'),
        invoke<number>('get_custom_packet_length', { protocol: 'tcp' }),
        invoke<number>('get_custom_packet_length', { protocol: 'udp' }),
      ])
      local.value = info
      savedTcpLength.value = customTcpLen
      savedUdpLength.value = customUdpLen
      interfaces.value = ifaces.length ? ifaces : [{ ip: info.ip, mac: info.mac, interfaceName: info.interfaceName, speedMbps: info.speedMbps }]
      // 服务端 IP 为空（或仍是旧版本内置默认值）时视为未手动设置，跟随所选网卡
      autoServerIp.value = !config.value.serverIp || config.value.serverIp === '192.168.1.100'
      applyNic(0)
      // 带宽限制默认取当前网卡最大带宽（-1 表示用户尚未设置过）
      if (config.value.bandwidth === -1) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
      // 网卡选择对话框只在主窗口弹出，避免多个窗口同时弹窗
      if (side.value === 'hub' && interfaces.value.length > 1) { nicSelected.value = 0; nicDialog.value = true }
      unlisten = await listen<BackendEvent>('test-event', (e) => handleEvent(e.payload))
    } catch (e) { log('WARN', `系统信息读取失败：${e}`) }
  } else {
    // 浏览器预览模式：无网卡信息，使用回退默认值
    if (!config.value.serverIp) config.value.serverIp = '127.0.0.1'
    if (config.value.bandwidth === -1) config.value.bandwidth = 100
  }
  log('INFO', 'LinkGauge 已就绪')
})
onUnmounted(() => { unlisten?.(); unlistenSync?.(); unlistenDock?.(); unlistenClose?.(); if (ticker) clearInterval(ticker) })
</script>

<template>
  <div class="app-shell" :class="['view-' + side, { 'no-summary': side === 'server' }]">
    <div class="titlebar">
      <div class="brand-icon">⌁</div>
      <h1>LinkGauge<template v-if="side !== 'hub'"> · {{ side === 'client' ? '客户端' : '服务端' }}</template></h1>
      <nav v-if="side === 'hub'" class="toolbar-actions"><button @click="importConfig"><Icon name="download" />导入配置</button><button @click="exportConfig"><Icon name="upload" />导出配置</button><button @click="infoDialog = { title: '引擎信息', message: '内置 riperf3 引擎（纯 Rust 实现 iperf3 协议，与官方 iperf3 互通）\n无需安装 iperf3，无外部依赖' }"><Icon name="settings" />设置</button><button @click="infoDialog = { title: '关于', message: 'LinkGauge v0.1.0\nRust + Tauri + Vue 3\n测试引擎：riperf3（MIT OR Apache-2.0）' }"><Icon name="info" />关于</button></nav>
      <nav v-else class="toolbar-actions"><button title="关闭此窗口并把标签放回主窗口" @click="dockBack"><Icon name="monitor" />⇱ 停靠回主窗口</button></nav>
    </div>
    <div class="workspace" :class="{ 'workspace-server': side === 'server' }">
      <ConfigPanel
        v-if="side === 'hub' && dockedTabs.length"
        :tabs="dockedTabs" :tab="activeTab" :config="config" :server-config="serverConfig" :items="items" :client-running="clientRunning" :server-running="serverRunning" :recovery="!!recovery" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:tab="activeTab = $event" @update:config="config = $event" @update:server-config="serverConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="stop" @start-server="startServer" @stop-server="stopServer" @clear="clearLogs" @pick-nic="openNicDialog" @save-custom-length="saveCustomLength" @detach="detachTab"
      />
      <ConfigPanel
        v-else-if="side === 'client'"
        detached="client" :tab="activeTab" :config="config" :server-config="serverConfig" :items="items" :client-running="clientRunning" :server-running="serverRunning" :recovery="!!recovery" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="config = $event" @update:server-config="serverConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="stop" @start-server="startServer" @stop-server="stopServer" @clear="clearLogs" @pick-nic="openNicDialog" @save-custom-length="saveCustomLength"
      />
      <ConfigPanel
        v-else-if="side === 'server'"
        detached="server" :tab="'server'" :config="config" :server-config="serverConfig" :items="items" :client-running="clientRunning" :server-running="serverRunning" :recovery="!!recovery" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="config = $event" @update:server-config="serverConfig = $event" @start="start" @stop="stop" @start-server="startServer" @stop-server="stopServer" @clear="clearLogs" @pick-nic="openNicDialog" @save-custom-length="saveCustomLength"
      />
      <div v-else class="panel config-panel dock-empty">
        <h2>标签页已分离</h2>
        <p>客户端 / 服务端已各自独立为窗口，可拖到另一块屏幕或分屏摆放；关闭子窗口即可收回标签。</p>
        <div class="dock-actions">
          <button class="primary" @click="requestDock('client')"><Icon name="monitor" />收回客户端标签</button>
          <button class="primary" @click="requestDock('server')"><Icon name="monitor" />收回服务端标签</button>
        </div>
      </div>
      <Dashboard v-if="side !== 'server'" :local="local" :config="config" :server-config="serverConfig" :current="current" :points="chartPoints" :progress="progress" :elapsed="elapsed" :summary="summary" :connected="connected" :server-running="serverRunning" :live="chartLive" :completed-label="completedLabel" />
      <StatusPanel :mode="side === 'server' ? 'logs' : 'full'" :items="items" :logs="logs" :server-running="serverRunning" @clear="clearLogs" @open-log-dir="openLogDir" @report="generateReport('html')" />
    </div>
    <ReportSummary v-if="side !== 'server'" :config="config" :summary="summary" @report="generateReport" @open-dir="openReportDir" />
    <div v-if="errorDialog || infoDialog" class="modal-backdrop" @click.self="errorDialog = null; infoDialog = null"><div class="modal"><button class="modal-close" @click="errorDialog = null; infoDialog = null">×</button><h2>{{ (errorDialog || infoDialog)?.title }}</h2><div class="modal-body"><span :class="['modal-symbol', errorDialog ? 'error' : 'info']">{{ errorDialog ? '×' : 'i' }}</span><p>{{ (errorDialog || infoDialog)?.message }}</p></div><div class="modal-actions"><button v-if="errorDialog" class="primary" @click="errorDialog = null; start()">重试</button><template v-if="infoDialog?.recovery"><button class="primary" @click="infoDialog = null; start()">恢复测试</button><button class="danger" @click="infoDialog = null; discardRecovery()">停止并重新开始</button></template><button v-if="!infoDialog?.recovery" @click="errorDialog = null; infoDialog = null">确定</button></div></div></div>
    <div v-if="nicDialog" class="modal-backdrop" @click.self="nicDialog = false; applyNic(0)"><div class="modal nic-modal"><button class="modal-close" @click="nicDialog = false; applyNic(0)">×</button><h2>选择本机网卡</h2><p class="nic-hint">检测到多个网卡，请选择测试使用的本机网卡：</p><div class="nic-list"><label v-for="(nic, index) in interfaces" :key="nic.ip" class="nic-option"><input type="radio" :value="index" v-model="nicSelected" /><span class="nic-name">{{ nic.interfaceName }}</span><span class="nic-ip">{{ nic.ip }}</span><span class="nic-speed">{{ nic.speedMbps ? nic.speedMbps + ' Mbps' : '速率未知' }}</span></label></div><div class="modal-actions"><button class="primary" @click="nicDialog = false; applyNic(nicSelected)">确定</button><button @click="nicDialog = false; applyNic(0)">取消（默认第一个）</button></div></div></div>
  </div>
</template>
