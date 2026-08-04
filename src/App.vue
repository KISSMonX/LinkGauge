<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { save } from '@tauri-apps/plugin-dialog'
import ConfigPanel from './components/ConfigPanel.vue'
import Dashboard from './components/Dashboard.vue'
import ServerDashboard from './components/ServerDashboard.vue'
import StatusPanel from './components/StatusPanel.vue'
import ReportSummary from './components/ReportSummary.vue'
import Icon from './components/Icon.vue'
import { useI18n, type Locale, type MessageKey } from './i18n'
import { theme, setTheme, type Theme } from './theme'
import type { BackendEvent, DockEvent, InterfaceInfo, LogEntry, MetricPoint, NetworkInfo, ServerConfig, SyncState, TestConfig, TestItem, TestSummary } from './types'

const { t, locale, setLocale } = useI18n()
const itemLabel = (id: string) => t(('cfg.item.' + id) as MessageKey)

const defaults: TestConfig = { mode: 'client', serverIp: '', port: 5201, duration: 30, parallel: 4, bandwidth: -1, packetLength: 131072, udpPacketLength: 8192, interval: 1 }
const serverDefaults: ServerConfig = { port: 5201, bindIp: '', interval: 1 }
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
/** 本次连接的客户端本地端口（控制连接建立时由后端广播） */
const clientLocalPort = ref(0)
const errorDialog = ref<{ title: string; message: string } | null>(null)
/** recovery 为 true 表示「发现未完成测试」弹窗，提供恢复/放弃两个选项 */
const infoDialog = ref<{ title: string; message: string; recovery?: boolean } | null>(null)
/** 设置弹窗：语言 / 主题 */
const settingsDialog = ref(false)
/** 停止确认弹窗：客户端停止测试 / 服务端停止服务（中英文跟随界面语言） */
const confirmDialog = ref<{ title: string; message: string; action: 'stop' | 'stop-server' } | null>(null)
interface RecoveryState { config: TestConfig; queue: string[]; nextIndex: number }
const recovery = ref<RecoveryState | null>(null)
const interfaces = ref<InterfaceInfo[]>([])
const savedTcpLength = ref(0)
const savedUdpLength = ref(0)
const nicDialog = ref(false)
const nicSelected = ref(0)
/** 网卡选择弹窗的目标：客户端「服务端 IP」还是服务端「绑定 IP」 */
const nicTarget = ref<'serverIp' | 'bindIp'>('serverIp')
/** 服务端 IP 尚未被用户手动修改时，重选网卡会同步更新它 */
const autoServerIp = ref(false)
/** 服务端独立统计（来自后端 status 事件广播，各窗口各自维护同一份数据） */
const serverUptime = ref(0)
const serverCompleted = ref(0)
const serverServing = ref(false)
const serverPoints = ref<MetricPoint[]>([])
/** 最近一次连接服务端的客户端地址（服务端概览「对端=客户端」） */
const serverPeerIp = ref('')
const serverPeerPort = ref(0)
/** 服务端视角只显示服务端自身日志（引擎日志 + 本窗口 UI 日志），与客户端日志独立 */
const serverLogs = computed(() => logs.value.filter((l) => l.module === 'server' || l.module === 'UI'))
/** 客户端视角只显示客户端自身日志（客户端任务 + 本窗口 UI 日志），与服务器日志独立 */
const clientLogs = computed(() => logs.value.filter((l) => l.module !== 'server'))
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
let unlistenSyncReq: UnlistenFn | undefined
let unlistenDock: UnlistenFn | undefined
let unlistenClose: UnlistenFn | undefined

/** 当前展示的是服务端视角：服务端分离窗口，或主窗口激活「服务端」标签（右侧概览/曲线/日志随之切换） */
const showServerView = computed(() => side.value === 'server' || (side.value === 'hub' && activeTab.value === 'server'))
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
    locale: locale.value,
    theme: theme.value,
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
  // 语言与主题跟随主窗口设置（持久化 + 应用到 DOM）
  setLocale(payload.locale)
  setTheme(payload.theme)
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
watch(locale, () => emitSync())
watch(theme, () => emitSync())

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
    errorDialog.value = { title: t('err.windowFailed'), message: String(e) }
  })
  log('INFO', t('tab.detachedLog', { side: s === 'client' ? t('common.client') : t('common.server') }))
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
  } catch (e) { log('WARN', t('log.dockNotifyFailed', { e: String(e) })) }
  try {
    await getCurrentWindow().destroy()
  } catch (e) { log('ERROR', t('log.dockFailed', { e: String(e) })) }
}

/** 应用指定序号的网卡：客户端模式更新本机信息与服务端 IP；服务端模式更新绑定 IP。
 *  explicit=true 表示用户在弹窗里明确选择（点「确定」），此时总是把服务端 IP 同步为所选网卡 IP */
function applyNic(index: number, explicit = false) {
  const nic = interfaces.value[index]
  if (!nic) return
  if (nicTarget.value === 'bindIp') {
    serverConfig.value = { ...serverConfig.value, bindIp: nic.ip }
    log('INFO', t('log.bindIpSet', { ip: nic.ip }))
    return
  }
  // 带宽限制仍是旧网卡默认值时，跟随新网卡速率（用户手动改过则保留）
  const bandwidthIsNicDefault = config.value.bandwidth === local.value.speedMbps
  local.value = { ...local.value, ip: nic.ip, mac: nic.mac, interfaceName: nic.interfaceName, speedMbps: nic.speedMbps }
  if (bandwidthIsNicDefault) config.value.bandwidth = nic.speedMbps > 0 ? nic.speedMbps : 0
  // 用户明确选择的网卡始终同步服务端 IP；启动/取消等默认应用仅在未手动设置过时跟随
  if (explicit || autoServerIp.value) config.value.serverIp = nic.ip
  log('INFO', t('log.nicSelected', { name: nic.interfaceName, ip: nic.ip }))
}
function openNicDialog(target: 'serverIp' | 'bindIp' = 'serverIp') {
  if (!interfaces.value.length || clientRunning.value) return
  nicTarget.value = target
  nicSelected.value = Math.max(0, interfaces.value.findIndex((i) => i.ip === local.value.ip))
  nicDialog.value = true
}
async function saveCustomLength(protocol: 'tcp' | 'udp', length: number) {
  const protocolName = protocol === 'tcp' ? t('common.tcp') : t('common.udp')
  if (!isTauri()) { if (protocol === 'tcp') { savedTcpLength.value = length; config.value.packetLength = length } else { savedUdpLength.value = length; config.value.udpPacketLength = length } log('INFO', t('log.customSaved', { protocol: protocolName, length })); return }
  try {
    await invoke('save_custom_packet_length', { protocol, length })
    if (protocol === 'tcp') { savedTcpLength.value = length; config.value.packetLength = length } else { savedUdpLength.value = length; config.value.udpPacketLength = length }
    log('INFO', t('log.customSavedFile', { protocol: protocolName, length }))
  } catch (error) { errorDialog.value = { title: t('err.saveFailed'), message: String(error) } }
}

function toggleItem(id: string) { const item = items.value.find((i) => i.id === id); if (item) item.enabled = !item.enabled }
function reset() { config.value = { ...defaults, mode: config.value.mode, serverIp: local.value.ip, bandwidth: local.value.speedMbps > 0 ? local.value.speedMbps : 0 }; log('INFO', t('log.reset')) }
function clearLogs() { logs.value = [] }
function validate() {
  if (config.value.mode === 'client' && !/^([a-z\d-]+\.)*[a-z\d-]+$/i.test(config.value.serverIp) && !/^\d{1,3}(\.\d{1,3}){3}$/.test(config.value.serverIp)) return t('err.serverIp')
  if (config.value.port < 1 || config.value.port > 65535) return t('err.port')
  if (config.value.duration < 1) return t('err.duration')
  return ''
}

async function start() {
  const savedRecovery = recovery.value
  // 恢复测试时参数以用户当前界面为准（应用启动时已从恢复状态加载过上次参数），
  // 不再用开始测试时的快照覆盖，避免用户修改的端口等参数被回退
  if (config.value.bandwidth < 0) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
  const invalid = validate(); if (invalid) { errorDialog.value = { title: t('err.paramError'), message: invalid }; return }
  const recoveredQueue = savedRecovery?.queue.slice(savedRecovery.nextIndex)
  // 恢复测试不覆盖用户当前勾选：启动时已按恢复队列初始化过勾选，
  // 用户之后手动调整的勾选在恢复/重试时保持原样；恢复队列只决定执行顺序
  // TCP / UDP 测试项可同时勾选，按列表顺序逐个执行
  const selected = items.value.filter((i) => i.enabled)
  if (!selected.length) { errorDialog.value = { title: t('err.noSelection'), message: t('err.noSelectionMsg') }; return }
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

/** 停止确认弹窗：点击「停止测试 / 停止服务」先询问用户，确认后才真正停止 */
function requestStop() {
  if (!clientRunning.value) return
  confirmDialog.value = { title: t('confirm.stopTitle'), message: t('confirm.stopMessage'), action: 'stop' }
}
function requestStopServer() {
  if (!serverRunning.value) return
  confirmDialog.value = { title: t('confirm.stopServerTitle'), message: t('confirm.stopServerMessage'), action: 'stop-server' }
}
async function confirmStop() {
  const action = confirmDialog.value?.action
  confirmDialog.value = null
  if (action === 'stop') await stop()
  else if (action === 'stop-server') await stopServer()
}

/** 启动 riperf3 服务端（独立于客户端任务队列，两者可同时运行） */
async function startServer() {
  if (serverRunning.value) return
  if (serverConfig.value.port < 1 || serverConfig.value.port > 65535) { errorDialog.value = { title: t('err.paramError'), message: t('err.port') }; return }
  if (serverConfig.value.interval < 1 || serverConfig.value.interval > 60) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverInterval') }; return }
  const bindTarget = serverConfig.value.bindIp.trim() || local.value.ip
  log('INFO', t('log.startServer', { addr: serverConfig.value.bindIp.trim() ? serverConfig.value.bindIp : t('sdash.allAdapters'), port: serverConfig.value.port }))
  if (!isTauri()) { serverRunning.value = true; log('INFO', t('log.previewServer')); return }
  // 新一轮服务端会话：清空上一次的概览统计与曲线
  serverUptime.value = 0; serverCompleted.value = 0; serverServing.value = false; serverPoints.value = []
  serverPeerIp.value = ''; serverPeerPort.value = 0
  try {
    serverSession.value = await invoke<string>('start_test', { request: { taskId: 'server', mode: 'server', protocol: 'tcp', serverIp: local.value.ip, localIp: local.value.ip, bindIp: serverConfig.value.bindIp, locale: locale.value, port: serverConfig.value.port, duration: 0, parallel: 0, bandwidth: 0, packetLength: 0, interval: serverConfig.value.interval } })
    serverRunning.value = true
  } catch (error) { log('ERROR', String(error)); errorDialog.value = { title: t('err.serverStartFailed'), message: String(error) } }
}
async function stopServer() {
  if (!serverRunning.value) return
  try { if (isTauri() && serverSession.value) await invoke('stop_test', { sessionId: serverSession.value }) } catch (e) { log('WARN', String(e)) }
  serverRunning.value = false; serverSession.value = ''
  log('INFO', t('log.stopServer'))
}

async function runNext() {
  queueIndex.value += 1
  if (queueIndex.value >= queue.value.length) { finishRun(true); return }
  const taskId = queue.value[queueIndex.value]
  const item = items.value.find((i) => i.id === taskId); if (item) item.status = 'running'
  progress.value = Math.round((queueIndex.value / Math.max(1, queue.value.length)) * 100)
  // 每个任务独立缓存：开始时清空实时数据，完成后快照到 completedPoints
  points.value = []
  log('INFO', t('log.startTask', { label: item ? itemLabel(item.id) : t('st.serverRunning') }))
  if (!isTauri()) { simulateTask(taskId); return }
  try {
    clientSession.value = await invoke<string>('start_test', { request: { taskId, localIp: local.value.ip, locale: locale.value, ...config.value } })
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
  // 服务端事件：独立于客户端队列，维护服务端运行状态与独立统计
  if (isServer) {
    if (event.type === 'status' && event.message) {
      try {
        const s = JSON.parse(event.message) as { uptime?: number; completed?: number; serving?: boolean; bandwidthMbps?: number | null; transferMb?: number; jitterMs?: number; lossPercent?: number; retransmits?: number; peerIp?: string; peerPort?: number }
        if (typeof s.uptime === 'number') serverUptime.value = s.uptime
        if (typeof s.completed === 'number') serverCompleted.value = s.completed
        serverServing.value = !!s.serving
        // 对端（客户端）地址：连接时携带；测试结束（客户端断开）后清除
        if (typeof s.peerIp === 'string' && s.peerIp) {
          serverPeerIp.value = s.peerIp
          serverPeerPort.value = s.peerPort ?? 0
        } else if (!s.serving) {
          serverPeerIp.value = ''
          serverPeerPort.value = 0
        }
        // 测试进行中且带间隔统计时，追加到服务端自己的实时曲线
        if (s.serving && typeof s.bandwidthMbps === 'number') {
          serverPoints.value.push({ second: s.uptime ?? 0, bandwidthMbps: s.bandwidthMbps, transferMb: s.transferMb ?? 0, jitterMs: s.jitterMs ?? 0, lossPercent: s.lossPercent ?? 0, retransmits: s.retransmits ?? 0 })
        }
      } catch { /* ignore */ }
    }
    if (event.type === 'complete') { serverRunning.value = false; serverSession.value = ''; serverServing.value = false; serverPeerIp.value = ''; serverPeerPort.value = 0; log('INFO', t('log.serverStopped', { reason: event.status === 'stopped' ? t('log.stoppedManual') : t('log.stoppedEnded') })) }
    if (event.type === 'error') { serverRunning.value = false; serverSession.value = ''; serverServing.value = false; serverPeerIp.value = ''; serverPeerPort.value = 0; log('ERROR', event.message || t('err.serverExited')); errorDialog.value = { title: t('err.serverError'), message: event.message || t('err.serverExited') } }
    return
  }
  // 客户端事件：停止或结束后的事件忽略
  if (!clientRunning.value) return
  if (event.type === 'status' && event.message) {
    try {
      const s = JSON.parse(event.message) as { localPort?: number }
      // 控制连接建立即标记已连接（不必等第一个指标输出周期），本地端口同时更新
      if (typeof s.localPort === 'number') {
        clientLocalPort.value = s.localPort
        if (!connected.value) { connected.value = true; log('INFO', t('log.connected')) }
      }
    } catch { /* ignore */ }
  }
  if (event.type === 'metric' && event.metric) { points.value.push(event.metric); connected.value = true; if (event.taskId === 'ping' && event.metric.jitterMs) summary.pingAverage = event.metric.jitterMs }
  if (event.type === 'complete') completeCurrent(event.status || 'success')
  if (event.type === 'error') failCurrent(event.message || t('err.execFailed'))
}

function completeCurrent(status: TestItem['status']) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = status
  if (status === 'success') summary.completed++
  // 快照本次测试的完整数据到内存缓存（测试完成后图表显示；退出应用即销毁）
  completedPoints.value = [...points.value]
  if (item) completedLabel.value = itemLabel(item.id)
  // 恢复进度只在任务成功时推进；失败/停止的任务保持原位，重试或恢复时会重新包含它，
  // 避免每次失败/恢复都吞掉一个测试项目
  if (recovery.value && status === 'success') { recovery.value.nextIndex = queueIndex.value + 1; localStorage.setItem('linkgauge-recovery', JSON.stringify(recovery.value)) }
  progress.value = Math.round(((queueIndex.value + 1) / queue.value.length) * 100)
  if (status === 'failed') { failCurrent(t('log.processExited')); return }
  // 仅驱动窗口启动下一项，避免多个窗口重复拉起同一个任务
  if (driver.value === ownLabel) void runNext()
}

function failCurrent(message: string) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = 'failed'
  completedPoints.value = [...points.value]
  if (item) completedLabel.value = itemLabel(item.id)
  log('ERROR', message); finishRun(false)
  const lastLog = summary.logPaths.at(-1)
  errorDialog.value = { title: t('err.alert'), message: lastLog ? `${message}\n\n${t('err.logFile', { path: lastLog })}` : message }
}
function finishRun(completed: boolean) { clientRunning.value = false; clientSession.value = ''; connected.value = false; if (completed) { progress.value = 100; recovery.value = null; localStorage.removeItem('linkgauge-recovery') } log('INFO', t('log.finish')) }
async function stop() { if (!clientRunning.value) return; completedPoints.value = [...points.value]; const item = current.value; if (item) completedLabel.value = itemLabel(item.id); try { if (isTauri() && clientSession.value) await invoke('stop_test', { sessionId: clientSession.value }) } catch (e) { log('WARN', String(e)) }; if (item) item.status = 'stopped'; /* 手动停止 = 主动结束本次测试：清除恢复状态，按钮回到「开始测试」，下次启动不再提示恢复（异常中断的恢复机制不受影响） */ recovery.value = null; localStorage.removeItem('linkgauge-recovery'); finishRun(false) }
/** 放弃未完成的测试队列并恢复默认全选，重新开始新一轮测试 */
function discardRecovery() {
  recovery.value = null
  localStorage.removeItem('linkgauge-recovery')
  items.value.forEach((item) => { item.enabled = true; item.status = 'waiting' })
  log('INFO', t('log.discardRecovery'))
}

const reportStamp = () => {
  const n = new Date()
  const p = (v: number) => String(v).padStart(2, '0')
  return `${n.getFullYear()}${p(n.getMonth() + 1)}${p(n.getDate())}${p(n.getHours())}${p(n.getMinutes())}${p(n.getSeconds())}`
}
/** 弹出系统保存对话框，让用户选择保存目录并自定义文件名，确认后生成报告 */
async function generateReport(format: 'html' | 'pdf' = 'html') {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.report', { format: format.toUpperCase() }) }; return }
  try {
    const dir = await invoke<string>('get_report_dir')
    const path = await save({
      title: t('report.saveTitle'),
      defaultPath: `${dir}\\linkgauge-report-${reportStamp()}.${format}`,
      filters: [{ name: format.toUpperCase(), extensions: [format] }]
    })
    if (!path) return // 用户取消
    const saved = await invoke<string>('generate_report', { request: { format, savePath: path, locale: locale.value, config: config.value, summary: { ...summary }, points: chartPoints.value, logs: logs.value } })
    infoDialog.value = { title: t('report.generated'), message: saved }
  } catch (error) { errorDialog.value = { title: t('err.reportFailed'), message: String(error) } }
}
/** 用系统文件管理器打开报告输出目录 */
async function openReportDir() {
  try {
    const dir = isTauri() ? await invoke<string>('open_report_dir') : t('preview.reportDir')
    log('INFO', t('log.reportDir', { dir }))
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}
/** 用系统文件管理器打开测试日志目录 */
async function openLogDir() {
  try {
    const dir = isTauri() ? await invoke<string>('open_log_dir') : t('preview.logDir')
    log('INFO', t('log.logDir', { dir }))
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}
/** 导出配置：弹出保存对话框，默认程序安装目录 config/config.json */
async function exportConfig() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.export') }; return }
  try {
    const dir = await invoke<string>('get_export_dir')
    const path = await save({ title: t('tb.exportConfig'), defaultPath: `${dir}\\config.json`, filters: [{ name: 'JSON', extensions: ['json'] }] })
    if (!path) return // 用户取消
    const saved = await invoke<string>('export_config', { path, config: JSON.stringify(config.value, null, 2) })
    infoDialog.value = { title: t('report.exported'), message: saved }
  } catch (error) { errorDialog.value = { title: t('err.exportFailed'), message: String(error) } }
}
function importConfig() { const input = document.createElement('input'); input.type = 'file'; input.accept = '.json'; input.onchange = async () => { const file = input.files?.[0]; if (!file) return; try { config.value = { ...defaults, ...JSON.parse(await file.text()) }; log('INFO', t('log.importOk')) } catch { errorDialog.value = { title: t('err.importFailed'), message: t('err.importInvalid') } } }; input.click() }

/** 设置弹窗：切换界面语言（默认英文）与主题外观（默认亮色），变更随 side-sync 同步到所有窗口 */
function onLocaleChange(event: Event) {
  setLocale((event.target as HTMLSelectElement).value as Locale)
}
function onThemeChange(event: Event) {
  setTheme((event.target as HTMLSelectElement).value as Theme)
}

onMounted(async () => {
  const saved = localStorage.getItem('linkgauge-config'); if (saved) try { const parsed = JSON.parse(saved); if (parsed.packetLength === 1024 || parsed.packetLength === 4096) parsed.packetLength = 131072 /* 旧默认值迁移为新默认 128KB */; config.value = { ...defaults, ...parsed } } catch { /* ignore */ }
  const savedServer = localStorage.getItem('linkgauge-server-config'); if (savedServer) try { serverConfig.value = { ...serverDefaults, ...JSON.parse(savedServer) } } catch { /* ignore */ }
  const unfinished = localStorage.getItem('linkgauge-recovery'); if (unfinished) try { recovery.value = JSON.parse(unfinished); config.value = { ...defaults, ...recovery.value!.config }; const remaining = recovery.value!.queue.slice(recovery.value!.nextIndex); items.value.forEach((item) => { item.enabled = remaining.includes(item.id); item.status = 'waiting' }); /* 恢复确认弹窗只在主窗口弹出，分离窗口静默加载状态（运行中的状态随后由同步事件更新） */ if (side.value === 'hub') infoDialog.value = { title: t('recover.title'), message: t('recover.message'), recovery: true } } catch { localStorage.removeItem('linkgauge-recovery') }
  if (isTauri()) {
    try {
      // 跨窗口同步：状态包广播 + 主窗口收回标签通知 + 子窗口被请求关闭
      unlistenSync = await listen<SyncState>('side-sync', (e) => applySync(e.payload))
      // 其他窗口请求当前状态（新分离的窗口启动时主动请求，避免错过状态变更而漏判事件）
      unlistenSyncReq = await listen('side-sync-request', () => emitSync())
      if (side.value === 'hub') {
        unlistenDock = await listen<DockEvent>('side-dock', (e) => dockTab(e.payload.side))
      } else {
        // 子窗口关闭（右上角 ✕）= 标签收回主窗口，而不是退出
        unlistenDock = await getCurrentWindow().onCloseRequested(async (event) => {
          event.preventDefault()
          try {
            await emit('side-dock', { side: side.value })
          } catch (e) { log('WARN', t('log.dockNotifyFailed', { e: String(e) })) }
          await getCurrentWindow().destroy()
        })
        unlistenClose = await listen<DockEvent>('side-close', (e) => { if (e.payload.side === side.value) void dockBack() })
        // 启动时请求一次完整状态（serverSession/clientRunning 等），随后的事件才能正确匹配会话
        void emit('side-sync-request')
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
    } catch (e) { log('WARN', t('log.sysInfoFailed', { e: String(e) })) }
  } else {
    // 浏览器预览模式：无网卡信息，使用回退默认值
    if (!config.value.serverIp) config.value.serverIp = '127.0.0.1'
    if (config.value.bandwidth === -1) config.value.bandwidth = 100
  }
  log('INFO', t('log.ready'))
})
onUnmounted(() => { unlisten?.(); unlistenSync?.(); unlistenSyncReq?.(); unlistenDock?.(); unlistenClose?.(); if (ticker) clearInterval(ticker) })
</script>

<template>
  <div class="app-shell" :class="['view-' + side, { 'no-summary': side === 'server' }]">
    <div class="titlebar">
      <div class="brand-icon">⌁</div>
      <h1>LinkGauge<template v-if="side !== 'hub'"> · {{ side === 'client' ? t('common.client') : t('common.server') }}</template></h1>
      <nav v-if="side === 'hub'" class="toolbar-actions"><button @click="importConfig"><Icon name="download" />{{ t('tb.importConfig') }}</button><button @click="exportConfig"><Icon name="upload" />{{ t('tb.exportConfig') }}</button><button @click="settingsDialog = true"><Icon name="settings" />{{ t('tb.settings') }}</button><button @click="infoDialog = { title: t('tb.about'), message: t('about.message') }"><Icon name="info" />{{ t('tb.about') }}</button></nav>
      <nav v-else class="toolbar-actions"><button :title="t('tb.dockBack')" @click="dockBack"><Icon name="monitor" />⇱ {{ t('tb.dockBack') }}</button></nav>
    </div>
    <div class="workspace">
      <ConfigPanel
        v-if="side === 'hub' && dockedTabs.length"
        :tabs="dockedTabs" :tab="activeTab" :config="config" :server-config="serverConfig" :items="items" :client-running="clientRunning" :server-running="serverRunning" :recovery="!!recovery" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:tab="activeTab = $event" @update:config="config = $event" @update:server-config="serverConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog" @pick-nic-server="openNicDialog('bindIp')" @save-custom-length="saveCustomLength" @detach="detachTab"
      />
      <ConfigPanel
        v-else-if="side === 'client'"
        detached="client" :tab="activeTab" :config="config" :server-config="serverConfig" :items="items" :client-running="clientRunning" :server-running="serverRunning" :recovery="!!recovery" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="config = $event" @update:server-config="serverConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog" @save-custom-length="saveCustomLength"
      />
      <ConfigPanel
        v-else-if="side === 'server'"
        detached="server" :tab="'server'" :config="config" :server-config="serverConfig" :items="items" :client-running="clientRunning" :server-running="serverRunning" :recovery="!!recovery" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="config = $event" @update:server-config="serverConfig = $event" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog()" @pick-nic-server="openNicDialog('bindIp')" @save-custom-length="saveCustomLength"
      />
      <div v-else class="panel config-panel dock-empty">
        <h2>{{ t('tab.detachedTitle') }}</h2>
        <p>{{ t('tab.detachedDesc') }}</p>
        <div class="dock-actions">
          <button class="primary" @click="requestDock('client')"><Icon name="monitor" />{{ t('tab.dockClient') }}</button>
          <button class="primary" @click="requestDock('server')"><Icon name="monitor" />{{ t('tab.dockServer') }}</button>
        </div>
      </div>
      <!-- 客户端视角：概览 + 实时曲线（主窗口客户端标签 / 客户端分离窗口） -->
      <Dashboard v-if="!showServerView" :local="local" :config="config" :server-config="serverConfig" :current="current" :points="chartPoints" :progress="progress" :elapsed="elapsed" :summary="summary" :connected="connected" :server-running="serverRunning" :live="chartLive" :completed-label="completedLabel" :local-port="clientLocalPort" />
      <!-- 服务端视角：服务端自身概览 + 服务端观测的实时曲线（与客户端数据独立），对端为客户端 -->
      <ServerDashboard v-else :bind-target="serverConfig.bindIp.trim() || t('sdash.allAdapters')" :port="serverConfig.port" :running="serverRunning" :uptime="serverUptime" :completed="serverCompleted" :serving="serverServing" :points="serverPoints" :peer-ip="serverPeerIp" :peer-port="serverPeerPort" />
      <!-- 客户端/服务端视角各自只显示自己的日志（引擎日志 + 本窗口 UI 日志） -->
      <StatusPanel :mode="showServerView ? 'logs' : 'full'" :items="items" :logs="showServerView ? serverLogs : clientLogs" :server-running="serverRunning" @clear="clearLogs" @open-log-dir="openLogDir" @report="generateReport('html')" />
    </div>
    <ReportSummary v-if="side !== 'server'" :config="config" :summary="summary" @report="generateReport" @open-dir="openReportDir" />
    <div v-if="errorDialog || infoDialog" class="modal-backdrop" @click.self="errorDialog = null; infoDialog = null"><div class="modal"><button class="modal-close" @click="errorDialog = null; infoDialog = null">×</button><h2>{{ (errorDialog || infoDialog)?.title }}</h2><div class="modal-body"><span :class="['modal-symbol', errorDialog ? 'error' : 'info']">{{ errorDialog ? '×' : 'i' }}</span><p>{{ (errorDialog || infoDialog)?.message }}</p></div><div class="modal-actions"><button v-if="errorDialog" class="primary" @click="errorDialog = null; start()">{{ t('common.retry') }}</button><template v-if="infoDialog?.recovery"><button class="primary" @click="infoDialog = null; start()">{{ t('recover.resume') }}</button><button class="danger" @click="infoDialog = null; discardRecovery()">{{ t('recover.discard') }}</button></template><button v-if="!infoDialog?.recovery" @click="errorDialog = null; infoDialog = null">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="confirmDialog" class="modal-backdrop" @click.self="confirmDialog = null"><div class="modal"><button class="modal-close" @click="confirmDialog = null">×</button><h2>{{ confirmDialog.title }}</h2><div class="modal-body"><span class="modal-symbol info">i</span><p>{{ confirmDialog.message }}</p></div><div class="modal-actions"><button @click="confirmDialog = null">{{ t('common.cancel') }}</button><button class="danger" @click="confirmStop">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="settingsDialog" class="modal-backdrop" @click.self="settingsDialog = false"><div class="modal"><button class="modal-close" @click="settingsDialog = false">×</button><h2>{{ t('settings.title') }}</h2><div class="settings-form"><label><span>{{ t('settings.language') }}</span><select :value="locale" @change="onLocaleChange($event)"><option value="en">English</option><option value="zh">中文</option></select></label><label><span>{{ t('settings.theme') }}</span><select :value="theme" @change="onThemeChange($event)"><option value="light">{{ t('settings.light') }}</option><option value="dark">{{ t('settings.dark') }}</option></select></label></div><p class="settings-note">{{ t('settings.engineNote') }}</p><div class="modal-actions"><button class="primary" @click="settingsDialog = false">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="nicDialog" class="modal-backdrop" @click.self="nicDialog = false; applyNic(0)"><div class="modal nic-modal"><button class="modal-close" @click="nicDialog = false; applyNic(0)">×</button><h2>{{ t('nic.title') }}</h2><p class="nic-hint">{{ t('nic.hint') }}</p><div class="nic-list"><label v-for="(nic, index) in interfaces" :key="nic.ip" class="nic-option"><input type="radio" :value="index" v-model="nicSelected" /><span class="nic-name">{{ nic.interfaceName }}</span><span class="nic-ip">{{ nic.ip }}</span><span class="nic-speed">{{ nic.speedMbps ? nic.speedMbps + ' Mbps' : t('nic.speedUnknown') }}</span></label></div><div class="modal-actions"><button class="primary" @click="nicDialog = false; applyNic(nicSelected, true)">{{ t('nic.confirm') }}</button><button @click="nicDialog = false; applyNic(0)">{{ t('nic.cancel') }}</button></div></div></div>
  </div>
</template>
