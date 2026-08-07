<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { emit, emitTo, listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { open, save } from '@tauri-apps/plugin-dialog'
import pkg from '../package.json'
import appIcon from './assets/app-icon.png'
import ConfigPanel from './components/ConfigPanel.vue'
import Dashboard from './components/Dashboard.vue'
import ServerDashboard from './components/ServerDashboard.vue'
import SshConsole from './components/SshConsole.vue'
import StatusPanel from './components/StatusPanel.vue'
import ReportSummary from './components/ReportSummary.vue'
import Icon from './components/Icon.vue'
import { dateLocale, itemLabel, useI18n, type Locale } from './i18n'
import { theme, setTheme, type Theme } from './theme'
import { clearTerminal } from './terminal'
import type { BackendEvent, DockEvent, InterfaceInfo, ItemHistory, LogEntry, MetricPoint, NetworkInfo, NetworkSnapshot, ServerConfig, SshConfig, SshEvent, SyncState, TestConfig, TestItem, TestSummary } from './types'
import { useServer } from './composables/useServer'
import { useSshConsole } from './composables/useSshConsole'

const { t, locale, setLocale } = useI18n()

const defaults: TestConfig = { mode: 'client', serverIp: '', port: 5201, duration: 30, parallel: 4, bandwidth: -1, packetLength: 131072, udpPacketLength: 1460, interval: 1, omitSecs: 0, windowKb: 0, cport: 0, ipVersion: 0, getServerOutput: false, transferMode: 'time', transferAmount: 100, dscp: 0, congestionAlgo: '', udpDontFragment: false, mptcp: false, authEnabled: false, authUsername: '', authPassword: '', authPublicKeyPath: '', authPkcs1Padding: false }
const serverDefaults: ServerConfig = { port: 5201, bindIp: '', interval: 1, authEnabled: false, authPrivateKeyPath: '', authUsersPath: '', authPkcs1Padding: false, idleTimeout: 0, maxDuration: 0, bitrateLimit: 0 }
const sshDefaults: SshConfig = { host: '', port: 22, username: '', authMethod: 'password', password: '', privateKeyPath: '', passphrase: '' }
const config = ref<TestConfig>({ ...defaults })
const serverConfig = ref<ServerConfig>({ ...serverDefaults })
const sshConfig = ref<SshConfig>({ ...sshDefaults })
// 产品当前只在 Linux 开放 MPTCP：Windows/macOS 原生内核不提供应用所需的
// IPPROTO_MPTCP，浏览器预览也必须遵循宿主操作系统，不能显示成可勾选。
const mptcpSupported = /Linux/i.test(navigator.userAgent) && !/Android/i.test(navigator.userAgent)
const congestionControlSupported = /Linux|FreeBSD/i.test(navigator.userAgent) && !/Android/i.test(navigator.userAgent)
/** 平台能力是本机事实，旧配置或其他窗口不能重新写入本平台不支持的参数。 */
function enforceConfigCapabilities(value: TestConfig): TestConfig {
  const mptcp = mptcpSupported ? value.mptcp : false
  const congestionAlgo = congestionControlSupported ? value.congestionAlgo : ''
  return mptcp === value.mptcp && congestionAlgo === value.congestionAlgo
    ? value
    : { ...value, mptcp, congestionAlgo }
}
const items = ref<TestItem[]>([
  { id: 'ping', label: 'Ping 连通性测试', protocol: 'ping', enabled: true, status: 'waiting' },
  { id: 'tcp-single', label: 'TCP 单向带宽', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'tcp-bidir', label: 'TCP 双向带宽', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'tcp-parallel', label: 'TCP 多并发流', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'udp-bandwidth', label: 'UDP 带宽', protocol: 'udp', enabled: true, status: 'waiting' },
  { id: 'udp-loss', label: 'UDP 抖动 / 丢包', protocol: 'udp', enabled: true, status: 'waiting' },
  { id: 'tcp-reverse', label: '反向测试（Reverse）', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'stress', label: '持续时间压力测试', protocol: 'tcp', enabled: true, status: 'waiting' },
  // —— 本次功能扩展新增的测试项（按量 / MPTCP / 禁止分片），默认勾选 ——
  { id: 'tcp-bytes', label: 'TCP 按量传输测试', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'udp-bytes', label: 'UDP 按量传输测试', protocol: 'udp', enabled: true, status: 'waiting' },
  { id: 'tcp-blocks', label: 'TCP 按块传输测试', protocol: 'tcp', enabled: true, status: 'waiting' },
  { id: 'tcp-mptcp', label: 'TCP MPTCP 多路径测试', protocol: 'tcp', enabled: mptcpSupported, status: 'waiting', supported: mptcpSupported },
  { id: 'udp-df', label: 'UDP 无分片（DF）测试', protocol: 'udp', enabled: true, status: 'waiting' }
])
/** 平台能力是本机事实，不能被旧窗口同步包中的 enabled/supported 覆盖。 */
function enforceItemCapabilities(values: TestItem[]) {
  let changed = false
  const enforced = values.map((item) => {
    if (item.id === 'tcp-mptcp') {
      const enabled = mptcpSupported && item.enabled
      if (enabled === item.enabled && item.supported === mptcpSupported) return item
      changed = true
      return { ...item, enabled, supported: mptcpSupported }
    }
    // 全局 DF 参数会作用于所有 UDP 项，专用测试此时重复且不能保留勾选。
    if (item.id === 'udp-df' && config.value.udpDontFragment && item.enabled) {
      changed = true
      return { ...item, enabled: false }
    }
    return item
  })
  return changed ? enforced : values
}
function updateConfig(value: TestConfig) {
  config.value = enforceConfigCapabilities(value)
  items.value = enforceItemCapabilities(items.value)
}
const local = ref<NetworkInfo>({ ip: '127.0.0.1', mac: '--', hostname: 'localhost', interfaceName: '默认网卡', speedMbps: 0 })
const logs = ref<LogEntry[]>([])
const points = ref<MetricPoint[]>([])
/** 最近一次已完成测试的完整数据缓存（仅内存，退出后销毁），完成后图表显示它 */
const completedPoints = ref<MetricPoint[]>([])
const completedLabel = ref('')
/** 各测试项目完成后的历史数据缓存（仅内存，退出应用即销毁），key = 项目 id；
 *  测试结束后点击队列中的项目，图表切换显示对应缓存的数据与曲线 */
const itemHistory = ref<Record<string, ItemHistory>>({})
/** 当前查看的历史项目 id（'' = 未选择，显示最近一次完成项） */
const selectedHistoryId = ref('')
/** 图表显示数据：测试进行中显示实时数据，否则显示所选历史项目（未选择时为最近一次完整测试）的数据 */
const chartPoints = computed(() => {
  if (current.value) return points.value
  const selected = selectedHistoryId.value ? itemHistory.value[selectedHistoryId.value] : undefined
  return selected ? selected.points : completedPoints.value
})
const chartLive = computed(() => !!current.value)
/** 图表标题的项目名：查看历史时用所选项目名，否则用最近一次完成项 */
const historyLabel = computed(() => selectedHistoryId.value ? itemLabel(selectedHistoryId.value) : completedLabel.value)
const clientRunning = ref(false)
const clientSession = ref('')
/** 后端活动队列会话：不参与跨窗口同步，防止旧快照改写 clientSession 后丢弃真实事件。 */
let activeClientQueueSession = ''
/** 已处理的后端 start 序号（取队列下标）：不受响应式同步包回退影响。 */
let activeQueueEventIndex = -1
const activeTab = ref<'client' | 'server'>('client')
const queue = ref<string[]>([])
const queueIndex = ref(-1)
const progress = ref(0)
/** 本轮测试开始时间戳（ms）：各窗口据此推算一致的已用时，随 side-sync 同步 */
const startedAt = ref(0)
/** 计时器心跳：每秒递增触发已用时重算（真正的计时来源是 startedAt） */
const nowTick = ref(0)
const elapsed = computed(() => { void nowTick.value; return startedAt.value ? Math.floor((Date.now() - startedAt.value) / 1000) : 0 })
const connected = ref(false)
/** 本次连接的客户端本地端口（控制连接建立时由后端广播） */
const clientLocalPort = ref(0)
/** 错误弹窗只用于展示失败原因，确认后关闭，不在测试过程中提供重试入口 */
const errorDialog = ref<{ title: string; message: string } | null>(null)
/** 信息弹窗（报告生成、预览模式等提示） */
const infoDialog = ref<{ title: string; message: string } | null>(null)
/** 设置弹窗：语言 / 主题 */
const settingsDialog = ref(false)
/** 「关于」弹窗 */
const aboutDialog = ref(false)
/** 关于页信息：桌面端由后端 get_app_info 提供（tauri.conf.json 版本 + 构建时 commit id），预览模式用回退值 */
const appInfo = ref<{ version: string; commit: string }>({ version: pkg.version, commit: 'dev' })
/** 关于页外部链接 */
const APP_PROJECT_URL = 'https://github.com/KISSMonX/LinkGauge'
const APP_ISSUES_URL = 'https://github.com/KISSMonX/LinkGauge/issues'
/** 停止确认弹窗：客户端停止测试 / 服务端停止服务 / 主窗口退出（中英文跟随界面语言） */
const confirmDialog = ref<{ title: string; message: string; action: 'stop' | 'stop-server' | 'exit' } | null>(null)
const interfaces = ref<InterfaceInfo[]>([])
const savedTcpLength = ref(0)
const savedUdpLength = ref(0)
const nicDialog = ref(false)
const nicSelected = ref(0)
/** 网卡选择弹窗的目标：客户端「服务端 IP」还是服务端「绑定 IP」 */
const nicTarget = ref<'serverIp' | 'bindIp'>('serverIp')
/** 服务端 IP 尚未被用户手动修改时，重选网卡会同步更新它 */
const autoServerIp = ref(false)
/** 服务端视角中间栏当前显示的视图（各窗口独立，不跨窗口同步） */
const serverView = ref<'overview' | 'ssh'>('overview')
/** 服务端视角只显示服务端自身日志（引擎日志 + SSH 会话日志 + 本窗口 UI 日志），与客户端日志独立 */
const serverLogs = computed(() => logs.value.filter((l) => l.module === 'server' || l.module === 'ssh' || l.module === 'UI'))
/** 客户端视角只显示客户端自身日志（客户端任务 + 本窗口 UI 日志），与服务器日志独立 */
const clientLogs = computed(() => logs.value.filter((l) => l.module !== 'server' && l.module !== 'ssh'))
let unlistenSsh: UnlistenFn | undefined
let ticker: number | undefined
let unlisten: UnlistenFn | undefined

// —— 多窗口支持：窗口角色（主窗口 hub / 分离的 client / server）+ 标签停靠状态 ——
const isTauri = () => '__TAURI_INTERNALS__' in window
const ownLabel = isTauri() ? getCurrentWebviewWindow().label : 'main'
/** 每次 WebView 创建时唯一；配合同步序号过滤自身回放和异步乱序包。 */
const syncInstance = `${ownLabel}-${Date.now()}-${Math.random().toString(36).slice(2)}`
let syncSequence = 0
const lastSyncSequence = new Map<string, number>()
/** hub = 主窗口（标签页容器）；client / server = 分离出的独立窗口 */
const side = ref<'hub' | 'client' | 'server'>(ownLabel === 'main' ? 'hub' : ownLabel === 'client' || ownLabel === 'server' ? ownLabel : 'hub')
/** 主窗口中当前停靠的标签页列表（顺序固定为客户端在前） */
const dockedTabs = ref<('client' | 'server')[]>(['client', 'server'])
/** 客户端运行状态的同步权威窗口：实际测试队列由 Rust 后端串行执行 */
const driver = ref('')
/** 驱动权单调版本：同一轮测试内每次合法接管递增，防止旧窗口自称驱动并回退队列 */
const driverRevision = ref(0)
let syncing = false
let syncApplyToken = 0
/** 分离窗口启动后收到首个远端状态同步前，禁止对外广播：
 *  否则启动瞬间会把空 points/logs/会话广播出去，覆盖其他窗口的实时数据 */
let stateReady = side.value === 'hub'
let unlistenSync: UnlistenFn | undefined
let unlistenSyncReq: UnlistenFn | undefined
let unlistenDock: UnlistenFn | undefined
let unlistenClose: UnlistenFn | undefined
let unlistenExit: UnlistenFn | undefined

/** 当前展示的是服务端视角：服务端分离窗口，或主窗口激活「服务端」标签（右侧概览/曲线/日志随之切换） */
const showServerView = computed(() => side.value === 'server' || (side.value === 'hub' && activeTab.value === 'server'))
const current = computed(() => items.value.find((i) => i.status === 'running'))
const summary = reactive<TestSummary>({ startedAt: '', completed: 0, total: 0, averageBandwidth: 0, maxBandwidth: 0, minBandwidth: 0, totalTransferMb: 0, pingAverage: 0, lossPercent: 0, jitterMs: 0, logPaths: [] })
const now = () => new Date().toLocaleTimeString(dateLocale(), { hour12: false })
const log = (level: LogEntry['level'], message: string, module = 'UI') => logs.value.push({ time: now(), level, module, message })
// ---- Composables: extracted state + operations (must be after isTauri/log declarations) ----
const {
  serverRunning, serverSession, serverUptime, serverCompleted, serverServing,
  serverPoints, serverPeerIp, serverPeerPort,
  refreshServerState, startServer, stopServer,
} = useServer(isTauri, log, errorDialog, t, locale, serverConfig, local)
const {
  sshSession, sshStatus, sshTerminal,
  pickPrivateKey, sshConnect, sshDisconnect, sshSend, sshResize,
  primeConsole, handleSshEvent,
} = useSshConsole(isTauri, log, errorDialog, infoDialog, t, sshConfig, serverView)

/** 选择服务端认证用的 RSA 私钥文件（对应 --rsa-private-key-path） */
async function pickServerAuthKey() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('srv.authKeyPick'), multiple: false, directory: false, filters: [{ name: t('cfg.authKeyFilter'), extensions: ['pem', 'key', 'crt'] }] })
    if (typeof path === 'string') { serverConfig.value = { ...serverConfig.value, authPrivateKeyPath: path }; log('INFO', t('log.authServerKeyPicked', { path }), 'server') }
  } catch (e) { errorDialog.value = { title: t('err.openDirFailed'), message: String(e) } }
}

/** 选择服务端认证用的授权用户文件（对应 --authorized-users-path） */
async function pickServerAuthUsers() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('srv.authUsersPick'), multiple: false, directory: false })
    if (typeof path === 'string') { serverConfig.value = { ...serverConfig.value, authUsersPath: path }; log('INFO', t('log.authUsersPicked', { path }), 'server') }
  } catch (e) { errorDialog.value = { title: t('err.openDirFailed'), message: String(e) } }
}

/** 图表旁指标区显示的汇总：查看某项目历史时按该项目数据推导（与实时 watch 同一套公式），否则用整体汇总 */
const viewSummary = computed<TestSummary>(() => {
  if (current.value || !selectedHistoryId.value) return summary
  const cached = itemHistory.value[selectedHistoryId.value]
  if (!cached) return summary
  const pts = cached.points
  const valid = pts.filter((p) => p.bandwidthMbps > 0)
  const last = pts.at(-1)
  return {
    ...summary,
    averageBandwidth: valid.length ? valid.reduce((a, b) => a + b.bandwidthMbps, 0) / valid.length : 0,
    maxBandwidth: valid.length ? Math.max(...valid.map((p) => p.bandwidthMbps)) : 0,
    minBandwidth: valid.length ? Math.min(...valid.map((p) => p.bandwidthMbps)) : 0,
    totalTransferMb: pts.reduce((a, b) => a + b.transferMb, 0),
    pingAverage: last?.jitterMs ?? 0,
    lossPercent: last?.lossPercent ?? 0,
    jitterMs: last?.jitterMs ?? 0,
  }
})

watch(points, (value) => {
  const valid = value.filter((p) => p.bandwidthMbps > 0)
  summary.averageBandwidth = valid.length ? valid.reduce((a, b) => a + b.bandwidthMbps, 0) / valid.length : 0
  summary.maxBandwidth = valid.length ? Math.max(...valid.map((p) => p.bandwidthMbps)) : 0
  summary.minBandwidth = valid.length ? Math.min(...valid.map((p) => p.bandwidthMbps)) : 0
  summary.totalTransferMb = value.reduce((a, b) => a + b.transferMb, 0)
  const last = value.at(-1); if (last) { summary.lossPercent = last.lossPercent; summary.jitterMs = last.jitterMs }
  // 实时曲线变化即时同步（整数组替换，各窗口内容一致），拖出的窗口持续跟上最新数据
  emitSync()
}, { deep: true })

watch(() => config.value.serverIp, (value) => { if (value && value !== local.value.ip) autoServerIp.value = false })
/** 认证密码不落盘：localStorage 与导出的配置 JSON 都会被用户复制/分享，
 *  明文密码不应留在里面。密码只在内存中、以及窗口间的 side-sync 里流转，
 *  重启应用后需要重新输入（用户名与公钥路径不是机密，照常保存） */
const withoutSecret = (value: TestConfig) => { const { authPassword: _omitted, ...rest } = value; return rest }
/** SSH 登录密码与私钥口令同样不落盘：主机 / 端口 / 用户名 / 私钥路径不是机密，照常保存 */
const withoutSshSecret = (value: SshConfig) => { const { password: _pwd, passphrase: _phrase, ...rest } = value; return rest }
/** 参数自动保存：任何修改即时写入本地存储，下次启动读取最近一次配置 */
watch(config, (value) => { localStorage.setItem('linkgauge-config', JSON.stringify(withoutSecret(value))); if (!syncing) emitSync() }, { deep: true })
/** 服务端参数本地持久化（与客户端配置分开保存） */
watch(serverConfig, (value) => { localStorage.setItem('linkgauge-server-config', JSON.stringify(value)); if (!syncing) emitSync() }, { deep: true })
/** SSH 连接参数本地持久化（不含密码与私钥口令） */
watch(sshConfig, (value) => { localStorage.setItem('linkgauge-ssh-config', JSON.stringify(withoutSshSecret(value))); if (!syncing) emitSync() }, { deep: true })

// —— 跨窗口状态同步：任何本地变更广播 side-sync，其他窗口应用（syncing 标志防回声循环） ——
function syncBundle(withLogs = false): SyncState {
  syncSequence += 1
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
    driverRevision: driverRevision.value,
    source: ownLabel,
    sourceInstance: syncInstance,
    syncSequence,
    savedTcpLength: savedTcpLength.value,
    savedUdpLength: savedUdpLength.value,
    summary: { ...summary },
    locale: locale.value,
    theme: theme.value,
    // 运行中的瞬时数据：拖出标签的新窗口据此恢复完整界面
    points: points.value,
    completedPoints: completedPoints.value,
    completedLabel: completedLabel.value,
    itemHistory: itemHistory.value,
    selectedHistoryId: selectedHistoryId.value,
    progress: progress.value,
    startedAt: startedAt.value,
    connected: connected.value,
    clientLocalPort: clientLocalPort.value,
    // 常规广播不带日志（空数组）：各窗口都监听同一 test-event 广播自行累积，日志
    // 是高频数据，随包整体替换会让陈旧副本反复截断显示（日志「停在某一段」）。
    // 仅窗口初始化应答（withLogs）携带一次完整历史；applySync 对空数组不替换。
    logs: withLogs ? logs.value.filter((l) => l.module !== 'UI') : [],
    serverUptime: serverUptime.value,
    serverCompleted: serverCompleted.value,
    serverServing: serverServing.value,
    serverPoints: serverPoints.value,
    serverPeerIp: serverPeerIp.value,
    serverPeerPort: serverPeerPort.value,
    // SSH 只同步参数与会话状态；控制台正文输出量大，各窗口自行监听 ssh-event 并拉取回放缓冲
    sshConfig: sshConfig.value,
    sshSession: sshSession.value,
    sshStatus: sshStatus.value,
  }
}
let syncQueued = false
let syncEmitChain: Promise<void> = Promise.resolve()
function emitSync() {
  if (!isTauri() || syncing || !stateReady) return
  if (syncQueued) return
  syncQueued = true
  queueMicrotask(() => {
    syncQueued = false
    if (syncing || !stateReady) return
    const payload = syncBundle()
    // 同一窗口的快照串行发送，避免多个 watcher 同时 emit 后跨异步边界乱序到达。
    syncEmitChain = syncEmitChain.then(() => emit('side-sync', payload)).catch(() => {})
  })
}
async function applySync(payload: SyncState) {
  // app.emit 会广播回发起窗口；自身快照可能在本地状态已前进后才返回，必须丢弃。
  if (payload.source === ownLabel) return
  const incomingInstance = payload.sourceInstance || payload.source
  const incomingSequence = payload.syncSequence ?? 0
  const lastSequence = lastSyncSequence.get(incomingInstance) ?? -1
  if (incomingSequence > 0 && incomingSequence <= lastSequence) return
  if (incomingSequence > 0) lastSyncSequence.set(incomingInstance, incomingSequence)
  const applyToken = ++syncApplyToken
  syncing = true
  // 新分离窗口只应收到当前驱动的运行中快照；若异常收到非驱动快照，保持未就绪，
  // 等待权威应答，避免第一次响应来自旧窗口时直接绑定错误驱动
  const firstSync = !stateReady
  const validDriverClaim = payload.source === payload.driver
  const incomingDriverRevision = payload.driverRevision ?? 0
  if (firstSync && payload.clientRunning && !validDriverClaim) {
    if (applyToken === syncApplyToken) syncing = false
    return
  }
  // 收到任何远端同步即视为已就绪（自己的广播在就绪前被抑制，此处收到的必为远端）
  stateReady = true
  const localRunId = startedAt.value
  const sameRun = payload.startedAt === localRunId
  // 新一轮只能由「source=driver」的发起窗口建立；startedAt 单调递增，旧运行包
  // 无法覆盖新运行。接管同样要求版本递增，旧窗口即使仍自称驱动也会被拒绝。
  const newRun = payload.clientRunning && payload.startedAt > localRunId && validDriverClaim
  const takeover = payload.clientRunning && sameRun && validDriverClaim && incomingDriverRevision > driverRevision.value
  const currentDriver = sameRun && payload.source === driver.value && payload.driver === driver.value && incomingDriverRevision === driverRevision.value
  // 停止状态同样只采纳当前同步权威窗口；所有窗口都会收到后端 queue-complete，
  // 非权威窗口不得用漏事件的旧 items（仍为 running）覆盖最终状态。
  const sameRunStop = sameRun && !payload.clientRunning && currentDriver
  const acceptClientState = firstSync || newRun || takeover || currentDriver || sameRunStop
  config.value = enforceConfigCapabilities(payload.config)
  serverConfig.value = payload.serverConfig
  local.value = payload.local
  // 后端服务端可能跨窗口重建继续运行；已知为运行中时，不接受其他窗口的陈旧
  // false 快照覆盖。真实停止由后端 complete/error 事件或状态查询确认。
  if (payload.serverRunning || !serverRunning.value) {
    serverRunning.value = payload.serverRunning
    serverSession.value = payload.serverSession
  }
  // 队列运行状态只采纳新一轮、当前驱动、较新接管或同轮停止；非驱动副本只同步
  // enabled，不能用陈旧 items / queueIndex / clientRunning 回退驱动窗口。
  // 已绑定后端队列后，start/metric/complete 才是运行状态的事实来源；来自另一
  // 窗口、但生成于事件处理前的合法旧快照也不能回退当前项。
  const preserveBackendState = !!activeClientQueueSession && payload.clientSession === activeClientQueueSession
  if (acceptClientState && !preserveBackendState) {
    clientRunning.value = payload.clientRunning
    items.value = enforceItemCapabilities(payload.items)
    progress.value = payload.progress
    points.value = payload.points
    completedPoints.value = payload.completedPoints
    itemHistory.value = payload.itemHistory
    clientSession.value = payload.clientSession
    queue.value = payload.queue
    queueIndex.value = payload.queueIndex
    driver.value = payload.driver
    driverRevision.value = incomingDriverRevision
    Object.assign(summary, payload.summary)
    completedLabel.value = payload.completedLabel
    startedAt.value = payload.startedAt
    connected.value = payload.connected
    clientLocalPort.value = payload.clientLocalPort
    if (payload.clientRunning && payload.clientSession) {
      activeClientQueueSession = payload.clientSession
      activeQueueEventIndex = payload.queueIndex
    }
  } else {
    items.value.forEach((item, i) => { const remote = payload.items[i]; if (remote) item.enabled = item.supported === false || (item.id === 'udp-df' && config.value.udpDontFragment) ? false : remote.enabled })
  }
  savedTcpLength.value = payload.savedTcpLength
  savedUdpLength.value = payload.savedUdpLength
  // 运行中的瞬时数据已按权威来源处理；选择状态属于界面状态，可跨窗口同步
  selectedHistoryId.value = payload.selectedHistoryId
  // 常规同步包不带日志（空数组），各窗口由 test-event 广播自行累积，不做替换；
  // 仅窗口初始化应答（withLogs）携带历史日志时整体合并一次
  if (payload.logs.length > 0) logs.value = [...logs.value.filter((l) => l.module === 'UI'), ...payload.logs]
  serverUptime.value = payload.serverUptime
  serverCompleted.value = payload.serverCompleted
  serverServing.value = payload.serverServing
  serverPoints.value = payload.serverPoints
  serverPeerIp.value = payload.serverPeerIp
  serverPeerPort.value = payload.serverPeerPort
  sshConfig.value = payload.sshConfig
  sshStatus.value = payload.sshStatus
  // 会话 id 变化会触发回放缓冲拉取，本窗口据此补齐连接建立以来的控制台内容
  sshSession.value = payload.sshSession
  // 语言与主题跟随主窗口设置（持久化 + 应用到 DOM）
  setLocale(payload.locale)
  setTheme(payload.theme)
  // 等本次 Vue 变更刷完再解除标志，避免应用远端状态触发的 watcher 再广播回去
  await nextTick()
  // 多个异步 applySync 可能交叠；只有最后开始应用的包可以解除广播抑制。
  if (applyToken === syncApplyToken) syncing = false
}
/** 批量监听：状态/会话等变化即时同步至所有子窗口。
 *  计时器只在客户端运行期间走动（已用时由 startedAt 推算，ticker 只负责触发重算）。 */
watch(clientRunning, (running) => {
  if (running) { if (ticker === undefined) ticker = window.setInterval(() => { nowTick.value += 1 }, 1000) }
  else if (ticker !== undefined) { clearInterval(ticker); ticker = undefined }
  if (!syncing) emitSync()
})
// 浅监听——简单值或对象引用变化时同步
for (const s of [serverRunning, clientSession, serverSession, queue, queueIndex, driver, driverRevision, local, savedTcpLength, savedUdpLength, locale, theme, completedPoints, completedLabel, progress, startedAt, connected, clientLocalPort, serverUptime, serverCompleted, serverServing, serverPeerIp, serverPeerPort, sshStatus, selectedHistoryId]) {
  watch(s, () => emitSync())
}
// 深监听——数组/字典内部变化时同步；日志由 test-event 直接广播，
// 不随每行变化再发送完整状态包（新窗口初始化时 syncBundle(true) 一次性携带已有日志）
for (const s of [items, summary, itemHistory, serverPoints]) {
  watch(s, () => emitSync(), { deep: true })
}
/** 会话建立后（无论由哪个窗口发起）拉取回放缓冲：
 *  既补齐 invoke 返回前就已到达的输出，也让后开的窗口拿到完整的控制台历史 */
watch(sshSession, (id) => { emitSync(); if (id) void primeConsole(id) })

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
  // 驱动窗口被收回时由主窗口接管同步权；测试队列始终由 Rust 后端继续执行。
  if (wasDetached && driver.value === s && clientRunning.value) {
    driverRevision.value += 1
    driver.value = ownLabel
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

function toggleItem(id: string) { const item = items.value.find((i) => i.id === id); if (item && item.supported !== false && !(id === 'udp-df' && config.value.udpDontFragment)) item.enabled = !item.enabled }
function reset() { config.value = { ...defaults, mode: config.value.mode, serverIp: local.value.ip, bandwidth: local.value.speedMbps > 0 ? local.value.speedMbps : 0 }; log('INFO', t('log.reset')) }
function clearLogs() { logs.value = [] }
function validate() {
  if (config.value.mode === 'client' && !/^([a-z\d-]+\.)*[a-z\d-]+$/i.test(config.value.serverIp) && !/^\d{1,3}(\.\d{1,3}){3}$/.test(config.value.serverIp)) return t('err.serverIp')
  if (config.value.port < 1 || config.value.port > 65535) return t('err.port')
  if (config.value.duration < 1) return t('err.duration')
  // 按量测试项（tcp-bytes / udp-bytes / tcp-blocks）强制按量模式：与全局
  // 按量模式同样要求数量大于 0、与预热互斥
  const amountItems = ['tcp-bytes', 'udp-bytes', 'tcp-blocks']
  const amountItemSelected = items.value.some((i) => amountItems.includes(i.id) && i.enabled)
  if (config.value.omitSecs > 0) {
    if (config.value.transferMode !== 'time' || amountItemSelected) return t('err.omitMode')
    if (config.value.omitSecs >= config.value.duration) return t('err.omitTooLong')
  }
  if ((config.value.transferMode !== 'time' || amountItemSelected) && config.value.transferAmount < 1) return t('err.transferAmount')
  if (config.value.dscp > 63) return t('err.dscpRange')
  if (config.value.windowKb > 16384) return t('err.windowTooLarge')
  if (config.value.cport > 65535) return t('err.cport')
  if (![0, 4, 6].includes(config.value.ipVersion)) return t('err.ipVersion')
  // 认证三要素缺一不可：iperf3 用服务端公钥加密「用户名+密码」，少任何一项都会被服务端拒绝
  if (config.value.authEnabled && (!config.value.authUsername.trim() || !config.value.authPassword || !config.value.authPublicKeyPath.trim())) return t('err.authIncomplete')
  return ''
}

/** 未启用认证时清空凭据后再下发，避免此前填过的用户名/密码被意外发送 */
const authPayload = () => (config.value.authEnabled ? {} : { authUsername: '', authPassword: '', authPublicKeyPath: '', authPkcs1Padding: false })
const clientRequest = (taskId: string) => ({ taskId, localIp: local.value.ip, locale: locale.value, runId: startedAt.value, ...config.value, ...authPayload() })

/** 选择服务端 RSA 公钥文件（iperf3 认证用，对应 --rsa-public-key-path） */
async function pickPublicKey() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('cfg.authKeyPick'), multiple: false, directory: false, filters: [{ name: t('cfg.authKeyFilter'), extensions: ['pem', 'crt', 'key', 'pub'] }] })
    if (typeof path === 'string') { config.value = { ...config.value, authPublicKeyPath: path }; log('INFO', t('log.authKeyPicked', { path })) }
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}

/** 打开外部链接（「关于」页的项目地址 / 提交反馈）：桌面端经后端调系统默认浏览器，预览模式用 window.open */
function openLink(url: string) {
  if (isTauri()) invoke('open_url', { url }).catch((e) => log('WARN', String(e)))
  else window.open(url, '_blank')
}

async function start() {
  // 重复开始守卫：后端另有客户端队列单例兜底，防止多窗口同时发起两轮测试。
  if (clientRunning.value) { log('WARN', t('log.alreadyRunning')); return }
  // 开始即全新一轮测试：以当前勾选的项目为队列，不涉及上次未完成测试的恢复
  config.value = enforceConfigCapabilities(config.value)
  if (config.value.bandwidth < 0) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
  const invalid = validate(); if (invalid) { errorDialog.value = { title: t('err.paramError'), message: invalid }; return }
  // TCP / UDP 测试项可同时勾选，按列表顺序逐个执行
  // IPC 边界前再次排除本平台不支持的项目，防止旧配置/旧窗口状态绕过禁用控件。
  items.value = enforceItemCapabilities(items.value)
  const selected = items.value.filter((i) => i.enabled && i.supported !== false)
  if (!selected.length) { errorDialog.value = { title: t('err.noSelection'), message: t('err.noSelectionMsg') }; return }
  items.value.forEach((i) => { if (i.enabled) i.status = 'waiting' })
  points.value = []; progress.value = 0; connected.value = false; startedAt.value = Math.max(Date.now(), startedAt.value + 1)
  selectedHistoryId.value = '' // 新一轮测试开始：图表回到实时模式
  summary.startedAt = new Date().toLocaleString(dateLocale(), { hour12: false }); summary.completed = 0; summary.total = selected.length; summary.logPaths = []
  queue.value = selected.map((i) => i.id); queueIndex.value = -1; clientRunning.value = true
  // 本窗口成为多窗口状态同步的权威来源；实际测试队列由 Rust 后端串行驱动。
  driverRevision.value = 1
  driver.value = ownLabel
  config.value.mode = 'client'
  if (!isTauri()) { await runNext(); return }
  activeClientQueueSession = ''
  activeQueueEventIndex = -1
  try {
    const sid = await invoke<string>('start_test_queue', { requests: queue.value.map(clientRequest) })
    if (clientRunning.value) { activeClientQueueSession = sid; clientSession.value = sid }
  } catch (error) {
    activeClientQueueSession = ''
    activeQueueEventIndex = -1
    if (queueIndex.value < 0) queueIndex.value = 0
    failCurrent(String(error), true)
  }
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
  else if (action === 'exit') await exitApp()
}

/** 主窗口退出确认后：结束整个应用（后端 exit_app 退出主进程，子窗口一并关闭） */
async function exitApp() {
  try { if (isTauri()) await invoke('exit_app') } catch (e) { log('ERROR', String(e)) }
}

/** 前端生成的队列级日志补写客户端运行日志文件：这类日志不在后端事件流里
 * （界面可见但文件没有），经后端 append_client_log 命令落入运行日志 */
function appendRunLog(level: string, message: string) {
  if (!isTauri()) return
  invoke('append_client_log', {
    request: { runId: startedAt.value, localIp: local.value.ip, serverIp: config.value.serverIp, locale: locale.value, level, message },
  }).catch((e) => log('WARN', `运行日志补写失败（${String(e)}）——旧构建未注册 append_client_log 命令时需要重启应用`))
}

async function runNext() {
  queueIndex.value += 1
  if (queueIndex.value >= queue.value.length) { finishRun(true); return }
  const taskId = queue.value[queueIndex.value]
  const item = items.value.find((i) => i.id === taskId); if (item) item.status = 'running'
  // 进度按「当前正在测试的项」显示：进入下一项即前进一格，与「开始{label}」日志
  // 同步。此前只在完成时更新，任务运行期间进度条冻结在上一项的完成值——表现为
  // 「进度卡在第 4 项、日志却显示已在测第 5 项」
  progress.value = Math.round(((queueIndex.value + 1) / Math.max(1, queue.value.length)) * 100)
  // 每个任务独立缓存：开始时清空实时数据，完成后快照到 completedPoints
  points.value = []
  log('INFO', t('log.startTask', { label: item ? itemLabel(item.id) : t('st.serverRunning') }))
  simulateTask(taskId)
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
  if (event.type === 'queue-complete' &&
      event.sessionId === (activeClientQueueSession || clientSession.value)) {
    try {
      const finalState = JSON.parse(event.message || '{}') as { items?: { taskId: string; status: TestItem['status'] }[] }
      let completed = 0
      for (const result of finalState.items ?? []) {
        const item = items.value.find((candidate) => candidate.id === result.taskId)
        if (item) item.status = result.status
        if (result.status === 'success') completed++
      }
      summary.completed = completed
    } catch (e) {
      log('WARN', `队列最终状态解析失败：${String(e)}`)
    }
    finishRun(event.status === 'success')
    return
  }
  // 后端是客户端队列的唯一驱动者；每项开始时先广播 start，所有窗口据此切换
  // 当前项。首个 start 可能早于 start_test_queue 的 invoke 返回，因此同时用它
  // 建立队列会话 ID，后续事件再严格按「会话 + 当前任务」匹配。
  if (event.type === 'start' && event.taskId !== 'server') {
    if (!activeClientQueueSession && !clientRunning.value) return
    if (activeClientQueueSession && event.sessionId !== activeClientQueueSession) return
    const index = queue.value.indexOf(event.taskId)
    if (index < 0 || index <= activeQueueEventIndex) return
    activeClientQueueSession = event.sessionId
    activeQueueEventIndex = index
    clientRunning.value = true
    clientSession.value = event.sessionId
    queueIndex.value = index
    const item = items.value.find((i) => i.id === event.taskId)
    if (item) item.status = 'running'
    progress.value = Math.round(((index + 1) / Math.max(1, queue.value.length)) * 100)
    points.value = []
    connected.value = false
    log('INFO', t('log.startTask', { label: item ? itemLabel(item.id) : event.taskId }))
    return
  }
  // 客户端本地端口状态事件：必须在会话匹配之前处理——同机测试 connect 极快，
  // 该事件可能早于 invoke('start_test') 的返回值（sessionId 尚未更新）到达，
  // 按 sessionId 匹配会被丢弃，而端口只在连接时发出一次，错过就无法显示。
  // 客户端任务的 taskId 不会是 'server'，据此与服务端心跳 status 区分。
  if (event.type === 'status' && event.message && event.taskId !== 'server') {
    try {
      const s = JSON.parse(event.message) as { localPort?: number }
      // 控制连接建立即标记已连接（不必等第一个指标输出周期），本地端口同时更新
      if (typeof s.localPort === 'number') {
        clientLocalPort.value = s.localPort
        if (!connected.value) { connected.value = true; log('INFO', t('log.connected')) }
      }
    } catch { /* ignore */ }
  }
  // start 事件已经建立队列会话并切换当前任务；其余事件严格双重匹配，上一项
  // 的尾部日志或重复完成事件不能误标下一项、解除下一项状态。
  const currentTaskId = queue.value[activeQueueEventIndex >= 0 ? activeQueueEventIndex : queueIndex.value]
  const eventQueueSession = activeClientQueueSession || clientSession.value
  const isClient = event.taskId !== 'server' && !!eventQueueSession &&
    event.taskId === currentTaskId && event.sessionId === eventQueueSession
  const isServer = event.taskId === 'server' && serverRunning.value
  if (!isClient && !isServer) {
    // 任务切换瞬间，上一任务的迟到事件（引擎报告器最后一次 flush 与 complete
    // 乱序，先 complete 后尾部日志）属正常现象，静默丢弃；仅对 taskId 不属于
    // 队列任何项的异常事件告警（排查事件串台用）
    if (clientRunning.value && event.taskId !== 'server' && event.type !== 'status' && !queue.value.includes(event.taskId)) {
      log('WARN', `收到不属于队列任何任务的事件：${event.type} 任务=${event.taskId} 会话=${event.sessionId.slice(0, 8)}`)
    }
    return
  }
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
      } catch (e) { log('WARN', `服务端状态解析失败：${e}（${event.message}）`) }
    }
    if (event.type === 'complete') { serverRunning.value = false; serverSession.value = ''; serverServing.value = false; serverPeerIp.value = ''; serverPeerPort.value = 0; log('INFO', t('log.serverStopped', { reason: event.status === 'stopped' ? t('log.stoppedManual') : t('log.stoppedEnded') })) }
    if (event.type === 'error') { serverRunning.value = false; serverSession.value = ''; serverServing.value = false; serverPeerIp.value = ''; serverPeerPort.value = 0; log('ERROR', event.message || t('err.serverExited')); errorDialog.value = { title: t('err.serverError'), message: event.message || t('err.serverExited') } }
    return
  }
  // 客户端事件：停止或结束后的事件忽略
  if (!clientRunning.value) return
  if (event.type === 'metric' && event.metric) { points.value.push(event.metric); connected.value = true; if (event.taskId === 'ping' && event.metric.jitterMs) summary.pingAverage = event.metric.jitterMs }
  if (event.type === 'summary' && event.metric) {
    const metric = event.metric
    // 少于两个区间的按量/按块任务无法形成折线，用全程平均值补成从 0 到实测
    // 结束时间的水平线；正常长测试只更新汇总，不污染真实区间曲线。
    if (event.taskId !== 'ping' && points.value.length < 2) {
      points.value = [
        { ...metric, second: 0, transferMb: 0 },
        { ...metric, second: Math.max(1, metric.second) },
      ]
    }
    summary.averageBandwidth = metric.bandwidthMbps
    summary.totalTransferMb = metric.transferMb
    summary.lossPercent = metric.lossPercent
    summary.jitterMs = metric.jitterMs
  }
  if (event.type === 'complete') completeCurrent(event.status || 'success')
  // 后端错误在发出事件前已经写入运行日志；这里只更新界面与队列，避免重复落盘
  if (event.type === 'error') failCurrent(event.message || t('err.execFailed'), event.fatal, false)
}

/** 把当前实时数据按项目 id 快照进历史缓存（仅内存，退出应用即销毁）；
 *  测试结束后点击队列中的项目即展示此缓存 */
function snapshotItem(id: string, status: TestItem['status']) {
  itemHistory.value = { ...itemHistory.value, [id]: { status, points: [...points.value] } }
}

function completeCurrent(status: TestItem['status']) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = status
  if (status === 'success') summary.completed++
  // 快照本次测试的完整数据到内存缓存（测试完成后图表显示；退出应用即销毁）
  completedPoints.value = [...points.value]
  snapshotItem(queue.value[queueIndex.value], status)
  if (item) completedLabel.value = itemLabel(item.id)
  // 每项完成的客户端日志：与「开始{label}」配对，队列推进的可见反馈。此前只有
  // 开始/失败有日志，快任务在日志里跳着走，叠加进度条冻结容易误判为卡死
  if (status === 'success') log('INFO', t('log.taskDone', { label: item ? itemLabel(item.id) : '' }))
  progress.value = Math.round(((queueIndex.value + 1) / queue.value.length) * 100)
  if (status === 'failed') { failCurrent(t('log.processExited')); return }
  // 桌面端下一项由 Rust 队列自动启动；预览模式仍使用本地模拟队列。
  if (!isTauri() && driver.value === ownLabel) void runNext()
  else if (queueIndex.value + 1 >= queue.value.length) finishRun(true)
}

function failCurrent(message: string, abort = false, persist = true) {
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = 'failed'
  completedPoints.value = [...points.value]
  snapshotItem(queue.value[queueIndex.value], 'failed')
  if (item) completedLabel.value = itemLabel(item.id)
  const logMessage = abort ? `${t('err.queueAborted')}：${message}` : message
  // invoke 异常、看门狗等纯前端失败没有后端日志事件，必须在队列推进前主动补写
  if (persist) appendRunLog('ERROR', logMessage)
  log('ERROR', logMessage)
  // 环境性失败（服务端未启动等）中止整个队列：剩余引擎项必然同样失败，继续只会
  // 产生一串无意义失败，由用户修复环境后重新开始测试
  if (abort) {
    finishRun(false)
  } else if (!isTauri() && driver.value === ownLabel) void runNext()
  else if (queueIndex.value + 1 >= queue.value.length) finishRun(true)
  const lastLog = summary.logPaths.at(-1)
  errorDialog.value = { title: t('err.alert'), message: lastLog ? `${(abort ? `${t('err.queueAborted')}\n\n` : '') + message}\n\n${t('err.logFile', { path: lastLog })}` : message }
}
function finishRun(completed: boolean) { clientRunning.value = false; clientSession.value = ''; connected.value = false; if (completed) { progress.value = 100 } log('INFO', t('log.finish')) }
async function stop() { if (!clientRunning.value) return; completedPoints.value = [...points.value]; const item = current.value; if (item) completedLabel.value = itemLabel(item.id); if (item) snapshotItem(item.id, 'stopped'); try { if (isTauri() && (activeClientQueueSession || clientSession.value)) await invoke('stop_test', { sessionId: activeClientQueueSession || clientSession.value }) } catch (e) { log('WARN', String(e)) }; if (item) item.status = 'stopped'; finishRun(false) }

const reportStamp = () => {
  const n = new Date()
  const p = (v: number) => String(v).padStart(2, '0')
  return `${n.getFullYear()}${p(n.getMonth() + 1)}${p(n.getDate())}${p(n.getHours())}${p(n.getMinutes())}${p(n.getSeconds())}`
}
/** HTML 使用保存对话框；PDF 复用同一 HTML，并交给系统打印窗口选择输出位置 */
async function generateReport(format: 'html' | 'pdf' = 'html') {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.report', { format: format.toUpperCase() }) }; return }
  try {
    // 报告按测试项目分组：包含全部有历史缓存的项目（固定顺序），每项输出数据曲线与数据表
    const reportItems = items.value
      .filter((i) => itemHistory.value[i.id])
      .map((i) => ({ label: itemLabel(i.id), status: itemHistory.value[i.id].status, points: itemHistory.value[i.id].points }))
    const request = { format, locale: locale.value, config: config.value, summary: { ...summary }, points: chartPoints.value, items: reportItems, logs: logs.value }
    if (format === 'pdf') {
      await invoke<string>('generate_report', { request })
      log('INFO', t('report.printOpened'))
      return
    }
    const dir = await invoke<string>('get_report_dir')
    const path = await save({
      title: t('report.saveTitle'),
      defaultPath: `${dir}\\linkgauge-report-${reportStamp()}.html`,
      filters: [{ name: 'HTML', extensions: ['html'] }]
    })
    if (!path) return // 用户取消
    const saved = await invoke<string>('generate_report', { request: { ...request, savePath: path } })
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
    // 导出的 JSON 常被复制/分享，认证密码不写进去（导入后需重新输入）
    const saved = await invoke<string>('export_config', { path, config: JSON.stringify(withoutSecret(config.value), null, 2) })
    infoDialog.value = { title: t('report.exported'), message: saved }
  } catch (error) { errorDialog.value = { title: t('err.exportFailed'), message: String(error) } }
}
function importConfig() { const input = document.createElement('input'); input.type = 'file'; input.accept = '.json'; input.onchange = async () => { const file = input.files?.[0]; if (!file) return; try { const parsed = JSON.parse(await file.text()); if (!parsed.transferAmount) parsed.transferAmount = 100 /* 旧配置迁移，同 localStorage 加载 */; updateConfig({ ...defaults, ...parsed }); log('INFO', t('log.importOk')) } catch { errorDialog.value = { title: t('err.importFailed'), message: t('err.importInvalid') } } }; input.click() }

/** 设置弹窗：切换界面语言（默认英文）与主题外观（默认亮色），变更随 side-sync 同步到所有窗口 */
function onLocaleChange(event: Event) {
  const value = (event.target as HTMLSelectElement).value as Locale
  setLocale(value)
  // 同步给后端：运行中会话的引擎日志（服务端心跳/客户端间隔等）立即改用新语言
  if (isTauri()) invoke('set_locale', { locale: value }).catch(() => {})
}
function onThemeChange(event: Event) {
  setTheme((event.target as HTMLSelectElement).value as Theme)
}

onMounted(async () => {
  const saved = localStorage.getItem('linkgauge-config'); if (saved) try { const parsed = JSON.parse(saved); if (parsed.packetLength === 1024 || parsed.packetLength === 4096) parsed.packetLength = 131072 /* 旧默认值迁移为新默认 128KB */; if (parsed.udpPacketLength === 8192) parsed.udpPacketLength = 1460 /* 旧默认 8KB 会触发 IP 分片，迁移为 iperf3 默认 1460 */; if (!parsed.transferAmount) parsed.transferAmount = 100 /* 旧默认 0 迁移：按量测试项默认勾选后需非零数量 */; updateConfig({ ...defaults, ...parsed }) } catch { /* ignore */ }
  const savedServer = localStorage.getItem('linkgauge-server-config'); if (savedServer) try { serverConfig.value = { ...serverDefaults, ...JSON.parse(savedServer) } } catch { /* ignore */ }
  const savedSsh = localStorage.getItem('linkgauge-ssh-config'); if (savedSsh) try { sshConfig.value = { ...sshDefaults, ...JSON.parse(savedSsh), password: '', passphrase: '' } } catch { /* ignore */ }
  if (isTauri()) {
    try {
      // 启动时把界面语言同步给后端（引擎日志按当前语言输出；切换语言时 onLocaleChange 再次同步）
      invoke('set_locale', { locale: locale.value }).catch(() => {})
      // 跨窗口同步：状态包广播 + 主窗口收回标签通知 + 子窗口被请求关闭
      unlistenSync = await listen<SyncState>('side-sync', (e) => applySync(e.payload))
      // 其他窗口请求当前状态（新分离的窗口启动时主动请求）：定向应答完整快照
      // （含历史日志，withLogs=true），不广播给其他窗口以免其日志被副本替换
      unlistenSyncReq = await listen<{ from: string }>('side-sync-request', (e) => {
        // 测试运行中由当前驱动、空闲时由主窗口提供唯一初始化快照；否则多个窗口
        // 同时应答，新窗口可能先采纳陈旧 queueIndex / driver，再参与广播造成回退。
        const authoritative = clientRunning.value ? driver.value === ownLabel : ownLabel === 'main'
        if (stateReady && authoritative) void emitTo(e.payload.from, 'side-sync', syncBundle(true))
      })
      if (side.value === 'hub') {
        unlistenDock = await listen<DockEvent>('side-dock', (e) => dockTab(e.payload.side))
        // 主窗口关闭（✕）= 弹退出确认框：确认后调用 exit_app 退出整个应用，取消则保持打开
        unlistenExit = await getCurrentWindow().onCloseRequested((event) => {
          event.preventDefault()
          const runningNote = clientRunning.value || serverRunning.value ? '\n' + t('confirm.exitRunningNote') : ''
          confirmDialog.value = { title: t('confirm.exitTitle'), message: t('confirm.exitMessage') + runningNote, action: 'exit' }
        })
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
        // 启动时请求完整状态（serverSession/clientRunning 等），随后的事件才能正确匹配会话。
        // 主窗口可能尚未完成初始化（stateReady 虽为 true，但网络快照 / 配置尚未拉取完毕），
        // 首次 emit 无响应时以指数退避重试最多 3 次，避免子窗口因错过初始 sync 而孤立。
        void (async () => {
          for (let attempt = 0; attempt < 3 && !stateReady; attempt++) {
            if (attempt > 0) await new Promise(r => setTimeout(r, 200 * Math.pow(2, attempt - 1)))
            emit('side-sync-request', { from: ownLabel })
            // 留给主窗口处理 + 回传 side-sync 的窗口
            await new Promise(r => setTimeout(r, 150))
          }
        })()
      }
      const [network, customTcpLen, customUdpLen] = await Promise.all([
        invoke<NetworkSnapshot>('get_network_snapshot'),
        invoke<number>('get_custom_packet_length', { protocol: 'tcp' }),
        invoke<number>('get_custom_packet_length', { protocol: 'udp' }),
      ])
      const { info, interfaces: ifaces } = network
      local.value = info
      savedTcpLength.value = customTcpLen
      savedUdpLength.value = customUdpLen
      invoke<{ version: string; commit: string }>('get_app_info').then((info) => { appInfo.value = info }).catch(() => {})
      interfaces.value = ifaces.length ? ifaces : [{ ip: info.ip, mac: info.mac, interfaceName: info.interfaceName, speedMbps: info.speedMbps }]
      // 服务端 IP 为空（或仍是旧版本内置默认值）时视为未手动设置，跟随所选网卡
      autoServerIp.value = !config.value.serverIp || config.value.serverIp === '192.168.1.100'
      applyNic(0)
      // 带宽限制默认取当前网卡最大带宽（-1 表示用户尚未设置过）
      if (config.value.bandwidth === -1) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
      // 网卡选择对话框只在主窗口弹出，避免多个窗口同时弹窗
      if (side.value === 'hub' && interfaces.value.length > 1) { nicSelected.value = 0; nicDialog.value = true }
      unlisten = await listen<BackendEvent>('test-event', (e) => handleEvent(e.payload))
      unlistenSsh = await listen<SshEvent>('ssh-event', (e) => handleSshEvent(e.payload))
      await refreshServerState(true)
    } catch (e) { log('WARN', t('log.sysInfoFailed', { e: String(e) })) }
  } else {
    // 浏览器预览模式：无网卡信息，使用回退默认值
    if (!config.value.serverIp) config.value.serverIp = '127.0.0.1'
    if (config.value.bandwidth === -1) config.value.bandwidth = 100
  }
  log('INFO', t('log.ready'))
})
onUnmounted(() => { unlisten?.(); unlistenSsh?.(); unlistenSync?.(); unlistenSyncReq?.(); unlistenDock?.(); unlistenClose?.(); unlistenExit?.(); if (ticker) clearInterval(ticker) })
</script>

<template>
  <div class="app-shell" :class="['view-' + side, { 'no-summary': side === 'server' }]">
    <div class="titlebar">
      <img class="brand-icon" :src="appIcon" alt="LinkGauge" />
      <h1>LinkGauge<template v-if="side !== 'hub'"> · {{ side === 'client' ? t('common.client') : t('common.server') }}</template></h1>
      <nav v-if="side === 'hub'" class="toolbar-actions"><button @click="importConfig"><Icon name="download" />{{ t('tb.importConfig') }}</button><button @click="exportConfig"><Icon name="upload" />{{ t('tb.exportConfig') }}</button><button @click="settingsDialog = true"><Icon name="settings" />{{ t('tb.settings') }}</button><button @click="aboutDialog = true"><Icon name="info" />{{ t('tb.about') }}</button></nav>
      <nav v-else class="toolbar-actions"><button :title="t('tb.dockBack')" @click="dockBack"><Icon name="monitor" />⇱ {{ t('tb.dockBack') }}</button></nav>
    </div>
    <div class="workspace">
      <ConfigPanel
        v-if="side === 'hub' && dockedTabs.length"
        :tabs="dockedTabs" :tab="activeTab" :config="config" :server-config="serverConfig" :ssh-config="sshConfig" :ssh-status="sshStatus" :items="items" :mptcp-supported="mptcpSupported" :congestion-control-supported="congestionControlSupported" :client-running="clientRunning" :server-running="serverRunning" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:tab="activeTab = $event" @update:config="updateConfig" @update:server-config="serverConfig = $event" @update:ssh-config="sshConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog" @pick-nic-server="openNicDialog('bindIp')" @pick-public-key="pickPublicKey" @pick-auth-key="pickServerAuthKey" @pick-auth-users="pickServerAuthUsers" @save-custom-length="saveCustomLength" @detach="detachTab" @ssh-connect="sshConnect" @ssh-disconnect="sshDisconnect" @pick-private-key="pickPrivateKey"
      />
      <ConfigPanel
        v-else-if="side === 'client'"
        detached="client" :tab="activeTab" :config="config" :items="items" :mptcp-supported="mptcpSupported" :congestion-control-supported="congestionControlSupported" :client-running="clientRunning" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="updateConfig" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="requestStop" @clear="clearLogs" @pick-nic="openNicDialog" @pick-public-key="pickPublicKey" @save-custom-length="saveCustomLength"
      />
      <ConfigPanel
        v-else-if="side === 'server'"
        detached="server" :tab="'server'" :server-config="serverConfig" :ssh-config="sshConfig" :ssh-status="sshStatus" :server-running="serverRunning"
        @update:server-config="serverConfig = $event" @update:ssh-config="sshConfig = $event" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic-server="openNicDialog('bindIp')" @pick-auth-key="pickServerAuthKey" @pick-auth-users="pickServerAuthUsers" @ssh-connect="sshConnect" @ssh-disconnect="sshDisconnect" @pick-private-key="pickPrivateKey"
      />
      <div v-else class="panel config-panel dock-empty">
        <h2>{{ t('tab.detachedTitle') }}</h2>
        <p>{{ t('tab.detachedDesc') }}</p>
        <div class="dock-actions">
          <button class="primary" @click="requestDock('client')"><Icon name="monitor" />{{ t('tab.dockClient') }}</button>
          <button class="primary" @click="requestDock('server')"><Icon name="monitor" />{{ t('tab.dockServer') }}</button>
        </div>
      </div>
      <!-- 服务端视角：概览（自身统计与观测曲线）与 SSH 远程控制台两个视图，用 v-show 保留各自状态 -->
      <template v-if="showServerView">
        <ServerDashboard
          v-show="serverView === 'overview'" :view="serverView" :ssh-status="sshStatus" @update:view="serverView = $event"
          :bind-target="serverConfig.bindIp.trim() || t('sdash.allAdapters')" :port="serverConfig.port" :running="serverRunning" :uptime="serverUptime" :completed="serverCompleted" :serving="serverServing" :points="serverPoints" :peer-ip="serverPeerIp" :peer-port="serverPeerPort"
        />
        <SshConsole
          v-show="serverView === 'ssh'" :view="serverView" :lines="sshTerminal.lines" :status="sshStatus" :host="sshConfig.host" :username="sshConfig.username" :server-port="serverConfig.port" :interval="serverConfig.interval"
          @update:view="serverView = $event" @send="sshSend" @clear="clearTerminal(sshTerminal)" @connect="sshConnect" @disconnect="sshDisconnect" @resize="sshResize"
        />
      </template>
      <!-- 客户端视角：概览 + 实时曲线（主窗口客户端标签 / 客户端分离窗口） -->
      <Dashboard v-else :local="local" :config="config" :server-config="serverConfig" :current="current" :points="chartPoints" :progress="progress" :elapsed="elapsed" :summary="viewSummary" :connected="connected" :server-running="serverRunning" :live="chartLive" :completed-label="historyLabel" :local-port="clientLocalPort" />
      <!-- 客户端/服务端视角各自只显示自己的日志（引擎日志 + 本窗口 UI 日志） -->
      <StatusPanel :mode="showServerView ? 'logs' : 'full'" :items="items" :logs="showServerView ? serverLogs : clientLogs" :server-running="serverRunning" :history="itemHistory" :history-selected="selectedHistoryId" :running="clientRunning" @select="selectedHistoryId = $event" @clear="clearLogs" @open-log-dir="openLogDir" @report="generateReport('html')" />
    </div>
    <ReportSummary v-if="side !== 'server'" :config="config" :summary="summary" @report="generateReport" @open-dir="openReportDir" />
    <div v-if="errorDialog || infoDialog" class="modal-backdrop" @click.self="errorDialog = null; infoDialog = null"><div class="modal"><button class="modal-close" @click="errorDialog = null; infoDialog = null">×</button><h2>{{ (errorDialog || infoDialog)?.title }}</h2><div class="modal-body"><span :class="['modal-symbol', errorDialog ? 'error' : 'info']">{{ errorDialog ? '×' : 'i' }}</span><p>{{ (errorDialog || infoDialog)?.message }}</p></div><div class="modal-actions"><button @click="errorDialog = null; infoDialog = null">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="confirmDialog" class="modal-backdrop" @click.self="confirmDialog = null"><div class="modal"><button class="modal-close" @click="confirmDialog = null">×</button><h2>{{ confirmDialog.title }}</h2><div class="modal-body"><span class="modal-symbol info">i</span><p>{{ confirmDialog.message }}</p></div><div class="modal-actions"><button @click="confirmDialog = null">{{ t('common.cancel') }}</button><button class="danger" @click="confirmStop">{{ confirmDialog.action === 'exit' ? t('confirm.exitButton') : t('common.confirm') }}</button></div></div></div>
    <div v-if="settingsDialog" class="modal-backdrop" @click.self="settingsDialog = false"><div class="modal"><button class="modal-close" @click="settingsDialog = false">×</button><h2>{{ t('settings.title') }}</h2><div class="settings-form"><label><span>{{ t('settings.language') }}</span><select :value="locale" @change="onLocaleChange($event)"><option value="en">English</option><option value="zh">中文</option></select></label><label><span>{{ t('settings.theme') }}</span><select :value="theme" @change="onThemeChange($event)"><option value="light">{{ t('settings.light') }}</option><option value="dark">{{ t('settings.dark') }}</option></select></label></div><p class="settings-note">{{ t('settings.engineNote') }}</p><div class="modal-actions"><button class="primary" @click="settingsDialog = false">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="nicDialog" class="modal-backdrop"><div class="modal nic-modal"><h2>{{ t('nic.title') }}</h2><p class="nic-hint">{{ t('nic.hint') }}</p><div class="nic-list"><label v-for="(nic, index) in interfaces" :key="nic.ip" class="nic-option"><input type="radio" :value="index" v-model="nicSelected" /><span class="nic-name">{{ nic.interfaceName }}</span><span class="nic-ip">{{ nic.ip }}</span><span class="nic-speed">{{ nic.speedMbps ? nic.speedMbps + ' Mbps' : t('nic.speedUnknown') }}</span></label></div><div class="modal-actions"><button class="primary" @click="nicDialog = false; applyNic(nicSelected, true)">{{ t('nic.confirm') }}</button><button @click="nicDialog = false">{{ t('nic.cancel') }}</button></div></div></div>
    <div v-if="aboutDialog" class="modal-backdrop" @click.self="aboutDialog = false"><div class="modal about-modal"><button class="modal-close" @click="aboutDialog = false">×</button><div class="about-header"><img class="about-logo" :src="appIcon" alt="LinkGauge" /><div><h2>LinkGauge</h2><p class="about-intro">{{ t('about.intro') }}</p></div></div><dl class="about-rows"><div class="about-row"><dt>{{ t('about.version') }}</dt><dd>v{{ appInfo.version }}</dd></div><div class="about-row"><dt>{{ t('about.commit') }}</dt><dd><code>{{ appInfo.commit }}</code></dd></div><div class="about-row"><dt>{{ t('about.author') }}</dt><dd>KISSMonX</dd></div><div class="about-row"><dt>{{ t('about.project') }}</dt><dd><button class="about-link" :title="APP_PROJECT_URL" @click="openLink(APP_PROJECT_URL)">github</button></dd></div></dl><div class="about-actions"><button class="primary" @click="openLink(APP_ISSUES_URL)">{{ t('about.feedback') }}</button></div><p class="about-engine">{{ t('about.engine') }}</p></div></div>
  </div>
</template>
