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
import { useI18n, type Locale, type MessageKey } from './i18n'
import { theme, setTheme, type Theme } from './theme'
import { clearTerminal, createTerminal, writeTerminal } from './terminal'
import type { BackendEvent, DockEvent, InterfaceInfo, ItemHistory, LogEntry, MetricPoint, NetworkInfo, ServerConfig, SshConfig, SshEvent, SshSnapshot, SshStatus, SyncState, TestConfig, TestItem, TestSummary } from './types'

const { t, locale, setLocale } = useI18n()
const itemLabel = (id: string) => t(('cfg.item.' + id) as MessageKey)

const defaults: TestConfig = { mode: 'client', serverIp: '', port: 5201, duration: 30, parallel: 4, bandwidth: -1, packetLength: 131072, udpPacketLength: 1460, interval: 1, omitSecs: 0, windowKb: 0, cport: 0, ipVersion: 0, getServerOutput: false, transferMode: 'time', transferAmount: 100, dscp: 0, congestionAlgo: '', udpDontFragment: false, mptcp: false, authEnabled: false, authUsername: '', authPassword: '', authPublicKeyPath: '', authPkcs1Padding: false }
const serverDefaults: ServerConfig = { port: 5201, bindIp: '', interval: 1, authEnabled: false, authPrivateKeyPath: '', authUsersPath: '', authPkcs1Padding: false, idleTimeout: 0, maxDuration: 0, bitrateLimit: 0 }
const sshDefaults: SshConfig = { host: '', port: 22, username: '', authMethod: 'password', password: '', privateKeyPath: '', passphrase: '' }
const config = ref<TestConfig>({ ...defaults })
const serverConfig = ref<ServerConfig>({ ...serverDefaults })
const sshConfig = ref<SshConfig>({ ...sshDefaults })
// Windows/macOS 内核无 IPPROTO_MPTCP：MPTCP 测试项不可用（勾选框禁用、默认不勾），
// 避免运行后报出「无法连接服务端」的误导性错误；预览模式（浏览器）保持可选
const mptcpSupported = !('__TAURI_INTERNALS__' in window) || /Linux/i.test(navigator.userAgent)
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
const serverRunning = ref(false)
const clientSession = ref('')
const serverSession = ref('')
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
/** 错误弹窗：retry=true 时才提供「重试」按钮（重试 = 重新开始客户端测试队列），
 *  服务端 / SSH / 文件操作类错误重试无意义，只给关闭 */
const errorDialog = ref<{ title: string; message: string; retry?: boolean } | null>(null)
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
/** 服务端独立统计（来自后端 status 事件广播，各窗口各自维护同一份数据） */
const serverUptime = ref(0)
const serverCompleted = ref(0)
const serverServing = ref(false)
const serverPoints = ref<MetricPoint[]>([])
/** 最近一次连接服务端的客户端地址（服务端概览「对端=客户端」） */
const serverPeerIp = ref('')
const serverPeerPort = ref(0)
/** 服务端视角只显示服务端自身日志（引擎日志 + SSH 会话日志 + 本窗口 UI 日志），与客户端日志独立 */
const serverLogs = computed(() => logs.value.filter((l) => l.module === 'server' || l.module === 'ssh' || l.module === 'UI'))
/** 客户端视角只显示客户端自身日志（客户端任务 + 本窗口 UI 日志），与服务器日志独立 */
const clientLogs = computed(() => logs.value.filter((l) => l.module !== 'server' && l.module !== 'ssh'))
// —— SSH 远程控制台（服务端视角）：在远端主机上操作 iperf3 服务端 ——
const sshSession = ref('')
const sshStatus = ref<SshStatus>('idle')
/** 控制台行缓冲：切换到概览标签时不销毁，故由 App 持有而非控制台组件 */
const sshTerminal = reactive(createTerminal())
/** 回放缓冲的结束偏移：偏移小于它的实时数据块已包含在回放内容里，需丢弃 */
const sshPrimed = ref(0)
/** 拉取回放缓冲期间到达的实时数据块（拉取完成后按偏移去重补写） */
let sshPending: SshEvent[] | null = null
/** 远端 PTY 尺寸（由控制台组件按可视区域测算） */
let sshCols = 120
let sshRows = 24
/** 服务端视角中间栏当前显示的视图（各窗口独立，不跨窗口同步） */
const serverView = ref<'overview' | 'ssh'>('overview')
let unlistenSsh: UnlistenFn | undefined
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
const now = () => new Date().toLocaleTimeString('zh-CN', { hour12: false })
const log = (level: LogEntry['level'], message: string, module = 'UI') => logs.value.push({ time: now(), level, module, message })
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
    source: ownLabel,
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
function emitSync() {
  if (!isTauri() || syncing || !stateReady) return
  void emit('side-sync', syncBundle())
}
async function applySync(payload: SyncState) {
  syncing = true
  // 首次同步（新分离窗口的初始化应答）视为可信快照，全量采纳；之后进入来源过滤
  const firstSync = !stateReady
  // 收到任何远端同步即视为已就绪（自己的广播在就绪前被抑制，此处收到的必为远端）
  stateReady = true
  config.value = payload.config
  serverConfig.value = payload.serverConfig
  local.value = payload.local
  clientRunning.value = payload.clientRunning
  serverRunning.value = payload.serverRunning
  // 队列运行中的展示状态（items 状态、进度、图表数据）只随驱动窗口同步：非驱动
  // 窗口也会处理 complete 事件（本地更新状态但不推进队列），其副本总是滞后——
  // 「已完成项仍标 running、新项还是 waiting」，整体替换会把驱动窗口的高亮/进度
  // 回退。勾选（enabled）任意窗口可改，正常采纳；clientRunning=false（停止后）
  // 恢复全量采纳，让 stopped/failed 标记与历史数据收敛。
  if (firstSync || payload.source === payload.driver || !clientRunning.value) {
    items.value = payload.items
    progress.value = payload.progress
    points.value = payload.points
    completedPoints.value = payload.completedPoints
    itemHistory.value = payload.itemHistory
  } else {
    items.value.forEach((item, i) => { const remote = payload.items[i]; if (remote) item.enabled = remote.enabled })
  }
  serverSession.value = payload.serverSession
  // 队列推进状态（queue/queueIndex/clientSession/driver）只采纳驱动窗口的同步：
  // 非驱动窗口收到 complete 也会执行 completeCurrent（更新状态但不推进队列），
  // 其广播携带旧 queueIndex；若驱动窗口采纳，队列会被「回退」到上一项——新任务
  // 事件全部被会话匹配丢弃、看门狗因当前任务不匹配而失效，表现为随机静默卡死
  // （日志停在「开始第 N+1 项」，进度条停在「第 N 项」）。clientRunning 保持
  // 全源采纳：任意窗口点「停止」都要立即生效。
  if (payload.source === payload.driver) {
    clientSession.value = payload.clientSession
    queue.value = payload.queue
    queueIndex.value = payload.queueIndex
    driver.value = payload.driver
  }
  savedTcpLength.value = payload.savedTcpLength
  savedUdpLength.value = payload.savedUdpLength
  Object.assign(summary, payload.summary)
  // 运行中的瞬时数据已在上方按驱动窗口来源处理；UI 日志本地保留，与远端引擎日志合并
  completedLabel.value = payload.completedLabel
  selectedHistoryId.value = payload.selectedHistoryId
  startedAt.value = payload.startedAt
  connected.value = payload.connected
  clientLocalPort.value = payload.clientLocalPort
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
  syncing = false
}
/** 运行状态/会话等变化即时同步；计时器只在运行期间走动（已用时由 startedAt 推算，ticker 只负责触发重算） */
watch(clientRunning, (running) => {
  if (running) { if (ticker === undefined) ticker = window.setInterval(() => { nowTick.value += 1 }, 1000) }
  else if (ticker !== undefined) { clearInterval(ticker); ticker = undefined }
  if (!syncing) emitSync()
})
watch(serverRunning, () => emitSync())
watch(clientSession, () => emitSync())
watch(serverSession, () => emitSync())
watch(queue, () => emitSync())
watch(queueIndex, () => emitSync())
watch(driver, () => emitSync())
watch(items, () => emitSync(), { deep: true })
watch(local, () => emitSync())
watch(savedTcpLength, () => emitSync())
watch(savedUdpLength, () => emitSync())
watch(summary, () => emitSync(), { deep: true })
watch(locale, () => emitSync())
watch(theme, () => emitSync())
// —— 运行中的瞬时数据：任何变化即时广播，所有窗口保持同一份数据 ——
watch(completedPoints, () => emitSync())
watch(completedLabel, () => emitSync())
watch(itemHistory, () => emitSync(), { deep: true })
watch(selectedHistoryId, () => emitSync())
watch(progress, () => emitSync())
watch(startedAt, () => emitSync())
watch(connected, () => emitSync())
watch(clientLocalPort, () => emitSync())
watch(logs, () => emitSync(), { deep: true })
watch(serverUptime, () => emitSync())
watch(serverCompleted, () => emitSync())
watch(serverServing, () => emitSync())
watch(serverPoints, () => emitSync(), { deep: true })
watch(serverPeerIp, () => emitSync())
watch(serverPeerPort, () => emitSync())
watch(sshStatus, () => emitSync())
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

/** 选择服务端 RSA 公钥文件（iperf3 认证用，对应 --rsa-public-key-path） */
async function pickPublicKey() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('cfg.authKeyPick'), multiple: false, directory: false, filters: [{ name: t('cfg.authKeyFilter'), extensions: ['pem', 'crt', 'key', 'pub'] }] })
    if (typeof path === 'string') { config.value = { ...config.value, authPublicKeyPath: path }; log('INFO', t('log.authKeyPicked', { path })) }
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}

// —— SSH 远程控制台：连接远端主机，在控制台里直接操作对端的 iperf3 服务端 ——

/** 选择服务端认证用的 RSA 私钥文件（对应 --rsa-private-key-path） */
async function pickServerAuthKey() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('srv.authKeyPick'), multiple: false, directory: false, filters: [{ name: t('cfg.authKeyFilter'), extensions: ['pem', 'key', 'crt'] }] })
    if (typeof path === 'string') { serverConfig.value = { ...serverConfig.value, authPrivateKeyPath: path }; log('INFO', t('log.authServerKeyPicked', { path }), 'server') }
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}

/** 选择服务端认证用的授权用户文件（对应 --authorized-users-path） */
async function pickServerAuthUsers() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('srv.authUsersPick'), multiple: false, directory: false })
    if (typeof path === 'string') { serverConfig.value = { ...serverConfig.value, authUsersPath: path }; log('INFO', t('log.authUsersPicked', { path }), 'server') }
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}

/** 选择 SSH 登录用的私钥文件 */
async function pickPrivateKey() {
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
  try {
    const path = await open({ title: t('ssh.keyPick'), multiple: false, directory: false })
    if (typeof path === 'string') { sshConfig.value = { ...sshConfig.value, privateKeyPath: path }; log('INFO', t('ssh.log.keyPicked', { path }), 'ssh') }
  } catch (error) { errorDialog.value = { title: t('err.openDirFailed'), message: String(error) } }
}

async function sshConnect() {
  if (sshStatus.value !== 'idle') return
  if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.ssh') }; return }
  // 新会话从空白控制台开始（断开后保留上一次输出，便于回看）
  clearTerminal(sshTerminal)
  sshPrimed.value = 0
  serverView.value = 'ssh'
  sshStatus.value = 'connecting'
  try {
    sshSession.value = await invoke<string>('ssh_connect', { request: { ...sshConfig.value, cols: sshCols, rows: sshRows } })
  } catch (error) {
    sshStatus.value = 'idle'
    log('ERROR', String(error), 'ssh')
    errorDialog.value = { title: t('ssh.failedTitle'), message: String(error) }
  }
}
async function sshDisconnect() {
  if (!sshSession.value) { sshStatus.value = 'idle'; return }
  try { if (isTauri()) await invoke('ssh_disconnect', { sessionId: sshSession.value }) } catch (e) { log('WARN', String(e), 'ssh') }
}
/** 向远端 shell 写入数据（命令自带换行，Ctrl+C 为 \x03） */
async function sshSend(data: string) {
  if (!sshSession.value || !isTauri()) return
  try { await invoke('ssh_send', { sessionId: sshSession.value, data }) } catch (e) { log('WARN', String(e), 'ssh') }
}
/** 控制台尺寸变化：同步远端 PTY 窗口大小，让远端命令按实际宽度换行 */
function sshResize(cols: number, rows: number) {
  if (cols === sshCols && rows === sshRows) return
  sshCols = cols; sshRows = rows
  if (sshSession.value && isTauri()) invoke('ssh_resize', { sessionId: sshSession.value, cols, rows }).catch(() => {})
}

/** 拉取会话快照写入控制台并对齐连接状态；拉取期间到达的实时数据先暂存，之后按偏移去重补写。
 *  它同时兜住了「connected 事件早于 ssh_connect 返回」的竞态：那一刻事件因会话 id 未知被丢弃，
 *  快照里的 connected 会把状态补回来 */
async function primeConsole(sessionId: string) {
  if (!isTauri()) return
  sshPending = []
  try {
    const snapshot = await invoke<SshSnapshot>('ssh_scrollback', { sessionId })
    if (sshSession.value !== sessionId) return
    clearTerminal(sshTerminal)
    writeTerminal(sshTerminal, snapshot.text)
    sshPrimed.value = snapshot.endOffset
    for (const event of sshPending) {
      if (event.sessionId === sessionId && (event.offset ?? 0) >= snapshot.endOffset) writeTerminal(sshTerminal, event.message || '')
    }
    if (snapshot.connected) sshStatus.value = 'connected'
  } catch {
    // 会话已结束（结束事件也可能早于会话 id 到达）：控制台内容保留，状态复位
    if (sshSession.value === sessionId) { sshSession.value = ''; sshStatus.value = 'idle' }
  }
  finally { sshPending = null }
}

function handleSshEvent(event: SshEvent) {
  if (!sshSession.value || event.sessionId !== sshSession.value) return
  if (event.type === 'data') {
    if (sshPending) { sshPending.push(event); return }
    // 回放缓冲已经包含的数据块直接丢弃，避免重复显示
    if ((event.offset ?? 0) < sshPrimed.value) return
    writeTerminal(sshTerminal, event.message || '')
    return
  }
  if (event.type === 'status') { if (event.message === 'connected' || event.message === 'connecting') sshStatus.value = event.message; return }
  if (event.type === 'log') { log(event.level || 'INFO', event.message || '', 'ssh'); return }
  if (event.type === 'closed' || event.type === 'error') {
    const message = event.message || ''
    writeTerminal(sshTerminal, `\r\n[${message}]\r\n`)
    log(event.type === 'error' ? 'ERROR' : 'INFO', message, 'ssh')
    sshStatus.value = 'idle'
    sshSession.value = ''
    sshPrimed.value = 0
  }
}

/** 打开外部链接（「关于」页的项目地址 / 提交反馈）：桌面端经后端调系统默认浏览器，预览模式用 window.open */
function openLink(url: string) {
  if (isTauri()) invoke('open_url', { url }).catch((e) => log('WARN', String(e)))
  else window.open(url, '_blank')
}

async function start() {
  // 重复开始守卫：队列运行中再次点击开始会在非驱动窗口产生第二个「驱动」，
  // 双 start_test 调用、事件交错（配合 applySync 的驱动来源过滤）
  if (clientRunning.value) { log('WARN', t('log.alreadyRunning')); return }
  // 开始即全新一轮测试：以当前勾选的项目为队列，不涉及上次未完成测试的恢复
  if (config.value.bandwidth < 0) config.value.bandwidth = local.value.speedMbps > 0 ? local.value.speedMbps : 0
  const invalid = validate(); if (invalid) { errorDialog.value = { title: t('err.paramError'), message: invalid, retry: true }; return }
  // TCP / UDP 测试项可同时勾选，按列表顺序逐个执行
  const selected = items.value.filter((i) => i.enabled)
  if (!selected.length) { errorDialog.value = { title: t('err.noSelection'), message: t('err.noSelectionMsg'), retry: true }; return }
  items.value.forEach((i) => { if (i.enabled) i.status = 'waiting' })
  points.value = []; progress.value = 0; connected.value = false; startedAt.value = Date.now()
  selectedHistoryId.value = '' // 新一轮测试开始：图表回到实时模式
  summary.startedAt = new Date().toLocaleString('zh-CN', { hour12: false }); summary.completed = 0; summary.total = selected.length; summary.logPaths = []
  queue.value = selected.map((i) => i.id); queueIndex.value = -1; clientRunning.value = true
  // 本窗口成为队列驱动者：只有它会在任务结束后启动下一项
  driver.value = ownLabel
  config.value.mode = 'client'
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
  else if (action === 'exit') await exitApp()
}

/** 主窗口退出确认后：结束整个应用（后端 exit_app 退出主进程，子窗口一并关闭） */
async function exitApp() {
  try { if (isTauri()) await invoke('exit_app') } catch (e) { log('ERROR', String(e)) }
}

/** 启动 riperf3 服务端（独立于客户端任务队列，两者可同时运行） */
async function startServer() {
  if (serverRunning.value) return
  if (serverConfig.value.port < 1 || serverConfig.value.port > 65535) { errorDialog.value = { title: t('err.paramError'), message: t('err.port') }; return }
  if (serverConfig.value.interval < 1 || serverConfig.value.interval > 60) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverInterval') }; return }
  if (serverConfig.value.idleTimeout > 86400 || serverConfig.value.maxDuration > 86400) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverLimitRange') }; return }
  if (serverConfig.value.bitrateLimit > 1000000) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverRateRange') }; return }
  if (serverConfig.value.authEnabled && (!serverConfig.value.authPrivateKeyPath.trim() || !serverConfig.value.authUsersPath.trim())) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverAuthIncomplete') }; return }
  const bindTarget = serverConfig.value.bindIp.trim() || local.value.ip
  log('INFO', t('log.startServer', { addr: serverConfig.value.bindIp.trim() ? serverConfig.value.bindIp : t('sdash.allAdapters'), port: serverConfig.value.port }))
  if (!isTauri()) { serverRunning.value = true; log('INFO', t('log.previewServer')); return }
  // 新一轮服务端会话：清空上一次的概览统计与曲线
  serverUptime.value = 0; serverCompleted.value = 0; serverServing.value = false; serverPoints.value = []
  serverPeerIp.value = ''; serverPeerPort.value = 0
  try {
    serverSession.value = await invoke<string>('start_test', { request: { taskId: 'server', mode: 'server', protocol: 'tcp', transferMode: 'time', serverIp: local.value.ip, localIp: local.value.ip, bindIp: serverConfig.value.bindIp, locale: locale.value, port: serverConfig.value.port, duration: 0, parallel: 0, bandwidth: 0, packetLength: 0, interval: serverConfig.value.interval, serverAuthEnabled: serverConfig.value.authEnabled, serverAuthPrivateKeyPath: serverConfig.value.authEnabled ? serverConfig.value.authPrivateKeyPath.trim() : '', serverAuthUsersPath: serverConfig.value.authEnabled ? serverConfig.value.authUsersPath.trim() : '', serverAuthPkcs1Padding: serverConfig.value.authPkcs1Padding, serverIdleTimeout: serverConfig.value.idleTimeout, serverMaxDuration: serverConfig.value.maxDuration, serverBitrateLimitMbps: serverConfig.value.bitrateLimit } })
    serverRunning.value = true
  } catch (error) { log('ERROR', String(error)); errorDialog.value = { title: t('err.serverStartFailed'), message: String(error) } }
}
async function stopServer() {
  if (!serverRunning.value) return
  try { if (isTauri() && serverSession.value) await invoke('stop_test', { sessionId: serverSession.value }) } catch (e) { log('WARN', String(e)) }
  serverRunning.value = false; serverSession.value = ''
  log('INFO', t('log.stopServer'))
}

// —— 任务看门狗：事件丢失时的兜底推进（防静默卡死）——
// 曾出现任务完成/失败事件未达前端时队列无限等待、日志图表静止的故障；
// 启动任务后若在期限内未收到 complete/error，强制标记失败并继续下一项
let watchdogTimer: number | undefined
/** 首事件探针定时器：任务启动后若 10 秒内收不到任何本任务事件，直接判失败（见 armWatchdog） */
let firstEventTimer: number | undefined
/** 看门狗武装时的队列游标：超时触发条件 = 队列尚未越过该游标（越过 = 任务已正常
 *  完成/失败并推进）。按游标而非当前任务 id 判定——即使游标被任何异常回退，
 *  超时也必然触发，任何卡死都无法静默持续 */
let watchdogArmedIndex = -1
let firstEventArmedIndex = -1
/** 任务类型感知的硬时限：从任务启动起算、不可被事件续期——任何卡死（invoke
 *  挂起、事件丢失、运行中途挂起）都必然超时，标记失败并继续下一项。按量/块
 *  任务若配置了带宽，按 传输量/带宽 估算预期时长 + 15 秒（下限 30 秒），避免
 *  慢速链路上的大传输量被误杀；ping 30 秒；按时长任务 duration+30 秒 */
function taskLimitMs(taskId: string) {
  if (taskId === 'tcp-bytes' || taskId === 'udp-bytes' || taskId === 'tcp-blocks') {
    const bw = config.value.bandwidth
    const expectedMs = bw > 0 ? (config.value.transferAmount * 8 / bw) * 1000 : 0
    return Math.max(30_000, expectedMs + 15_000)
  }
  if (taskId === 'ping') return 30_000
  return Math.max((config.value.duration + 30) * 1000, 30_000)
}
function armWatchdog(taskId: string) {
  clearTimeout(watchdogTimer)
  const limitMs = taskLimitMs(taskId)
  watchdogArmedIndex = queueIndex.value
  watchdogTimer = window.setTimeout(() => {
    if (!clientRunning.value || queueIndex.value > watchdogArmedIndex) return
    log('ERROR', `任务超时未收到完成事件（看门狗）：${taskId}`)
    failCurrent(`任务 ${taskId} 超过 ${Math.round(limitMs / 1000)} 秒未完成（超时，标记失败并继续下一项）`)
  }, limitMs)
  // 首事件探针：后端任务一旦启动会毫秒级发出「执行」事件；10 秒内没有任何本
  // 任务事件基本可断定 invoke 丢失或事件通道异常（「开始第 N 项后连执行日志
  // 都没有」的卡死形态）——直接判失败继续队列；迟到事件由任务匹配安全丢弃，
  // 不会误伤
  clearTimeout(firstEventTimer)
  firstEventArmedIndex = queueIndex.value
  firstEventTimer = window.setTimeout(() => {
    if (!clientRunning.value || queueIndex.value > firstEventArmedIndex) return
    failCurrent(t('log.taskNoStart', { task: itemLabel(taskId) }))
  }, 10_000)
}
function disarmWatchdog() { clearTimeout(watchdogTimer); clearTimeout(firstEventTimer) }

let passiveTimer: number | undefined
/** 驱动死亡守卫（仅主窗口）：驱动窗口被强杀（未走 side-dock 收回流程）或卡死时
 *  其同步停滞、队列无人推进。主窗口对当前任务同步武装被动看门狗：期限内驱动
 *  未推进（queueIndex 未变）且任务仍标记运行中，则接管驱动角色并判失败继续。
 *  看门狗超时通常由驱动窗口自身的计时器处理，此处只在驱动失效时兜底 */
function armPassiveWatchdog(taskId: string) {
  clearTimeout(passiveTimer)
  const limitMs = taskLimitMs(taskId)
  const armedIndex = queueIndex.value
  passiveTimer = window.setTimeout(() => {
    if (side.value !== 'hub' || !clientRunning.value || driver.value === ownLabel) return
    if (queueIndex.value > armedIndex) return
    const cur = items.value.find((i) => i.id === taskId)
    if (!cur || cur.status !== 'running') return
    driver.value = ownLabel
    failCurrent(t('log.driverDead', { driver: driver.value, task: itemLabel(taskId) }))
  }, limitMs)
}
watch([clientRunning, queueIndex, driver], () => {
  if (!clientRunning.value || driver.value === ownLabel || side.value !== 'hub') { clearTimeout(passiveTimer); return }
  const taskId = queue.value[queueIndex.value]
  if (taskId) armPassiveWatchdog(taskId)
})

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
  if (!isTauri()) { simulateTask(taskId); return }
  // 看门狗必须在 invoke 之前武装：若 invoke 挂起（后端无响应），队列会无限期
  // 卡在当前项且没有任何事件触发兜底——「卡在第 N 项、日志不刷新」的最后一条
  // 静默路径。invoke 正常返回后由其后的 complete/error 事件解除。
  armWatchdog(taskId)
  try {
    const sid = await invoke<string>('start_test', { request: { taskId, localIp: local.value.ip, locale: locale.value, runId: startedAt.value, ...config.value, ...authPayload() } })
    // invoke 挂起期间看门狗可能已判失败并推进队列，迟到的会话 id 不能覆盖新任务
    if (queue.value[queueIndex.value] === taskId) clientSession.value = sid
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
  // 会话匹配：任务极快完成时（如按量测试在回环上毫秒级传完），complete/日志
  // 事件可能早于 invoke('start_test') 返回（clientSession 尚未更新）到达，
  // 按 sessionId 匹配会被丢弃、队列卡死（AGENTS.md 的 loopback 时序问题——
  // localPort 有特例处理，普通事件没有）。双条件匹配：当前队列任务 id 或
  // clientSession 任一命中即属当前客户端任务；两者都是串行单会话，上一个
  // 任务的迟到事件会被同时过滤（taskId 与 sessionId 都不再指向它）
  const currentTaskId = queue.value[queueIndex.value]
  const isClient = event.taskId !== 'server' && clientRunning.value &&
    ((!currentTaskId || event.taskId === currentTaskId) || event.sessionId === clientSession.value)
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
  // 首事件探针解除：收到当前任务的任何事件（执行/指标/端口状态等）即视为启动成功
  if (event.taskId === currentTaskId) clearTimeout(firstEventTimer)
  if (event.type === 'metric' && event.metric) { points.value.push(event.metric); connected.value = true; if (event.taskId === 'ping' && event.metric.jitterMs) summary.pingAverage = event.metric.jitterMs }
  if (event.type === 'complete') completeCurrent(event.status || 'success')
  if (event.type === 'error') failCurrent(event.message || t('err.execFailed'), event.fatal)
}

/** 把当前实时数据按项目 id 快照进历史缓存（仅内存，退出应用即销毁）；
 *  测试结束后点击队列中的项目即展示此缓存 */
function snapshotItem(id: string, status: TestItem['status']) {
  itemHistory.value = { ...itemHistory.value, [id]: { status, points: [...points.value] } }
}

function completeCurrent(status: TestItem['status']) {
  disarmWatchdog()
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
  // 仅驱动窗口启动下一项，避免多个窗口重复拉起同一个任务
  if (driver.value === ownLabel) void runNext()
}

function failCurrent(message: string, abort = false) {
  disarmWatchdog()
  const item = items.value.find((i) => i.id === queue.value[queueIndex.value]); if (item) item.status = 'failed'
  completedPoints.value = [...points.value]
  snapshotItem(queue.value[queueIndex.value], 'failed')
  if (item) completedLabel.value = itemLabel(item.id)
  log('ERROR', abort ? `${t('err.queueAborted')}：${message}` : message)
  // 环境性失败（服务端未启动等）中止整个队列：剩余引擎项必然同样失败，继续只会
  // 产生一串无意义失败，由用户修复环境后重新开始测试
  if (abort) {
    finishRun(false)
  } else {
    // 单项失败不中止队列：标记 failed 后继续下一项（失败原因保留在弹窗与日志/报告中）。
    // 曾因 finishRun(false) 中止整个队列——默认队列里 tcp-mptcp 在不支持的平台上必然
    // 失败，表现为「卡在第 N 项、日志不刷新」（队列已停但 UI 无提示）
    if (driver.value === ownLabel) void runNext()
  }
  const lastLog = summary.logPaths.at(-1)
  errorDialog.value = { title: t('err.alert'), message: lastLog ? `${(abort ? `${t('err.queueAborted')}\n\n` : '') + message}\n\n${t('err.logFile', { path: lastLog })}` : message, retry: !abort }
}
function finishRun(completed: boolean) { disarmWatchdog(); clientRunning.value = false; clientSession.value = ''; connected.value = false; if (completed) { progress.value = 100 } log('INFO', t('log.finish')) }
async function stop() { if (!clientRunning.value) return; completedPoints.value = [...points.value]; const item = current.value; if (item) completedLabel.value = itemLabel(item.id); if (item) snapshotItem(item.id, 'stopped'); try { if (isTauri() && clientSession.value) await invoke('stop_test', { sessionId: clientSession.value }) } catch (e) { log('WARN', String(e)) }; if (item) item.status = 'stopped'; finishRun(false) }

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
    // 报告按测试项目分组：包含全部有历史缓存的项目（固定顺序），每项输出数据曲线与数据表
    const reportItems = items.value
      .filter((i) => itemHistory.value[i.id])
      .map((i) => ({ label: itemLabel(i.id), status: itemHistory.value[i.id].status, points: itemHistory.value[i.id].points }))
    const saved = await invoke<string>('generate_report', { request: { format, savePath: path, locale: locale.value, config: config.value, summary: { ...summary }, points: chartPoints.value, items: reportItems, logs: logs.value } })
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
function importConfig() { const input = document.createElement('input'); input.type = 'file'; input.accept = '.json'; input.onchange = async () => { const file = input.files?.[0]; if (!file) return; try { const parsed = JSON.parse(await file.text()); if (!parsed.transferAmount) parsed.transferAmount = 100 /* 旧配置迁移，同 localStorage 加载 */; config.value = { ...defaults, ...parsed }; log('INFO', t('log.importOk')) } catch { errorDialog.value = { title: t('err.importFailed'), message: t('err.importInvalid') } } }; input.click() }

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
  const saved = localStorage.getItem('linkgauge-config'); if (saved) try { const parsed = JSON.parse(saved); if (parsed.packetLength === 1024 || parsed.packetLength === 4096) parsed.packetLength = 131072 /* 旧默认值迁移为新默认 128KB */; if (parsed.udpPacketLength === 8192) parsed.udpPacketLength = 1460 /* 旧默认 8KB 会触发 IP 分片，迁移为 iperf3 默认 1460 */; if (!parsed.transferAmount) parsed.transferAmount = 100 /* 旧默认 0 迁移：按量测试项默认勾选后需非零数量 */; config.value = { ...defaults, ...parsed } } catch { /* ignore */ }
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
      unlistenSyncReq = await listen<{ from: string }>('side-sync-request', (e) => { if (stateReady) void emitTo(e.payload.from, 'side-sync', syncBundle(true)) })
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
        // 启动时请求一次完整状态（serverSession/clientRunning 等），随后的事件才能正确匹配会话
        void emit('side-sync-request', { from: ownLabel })
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
        :tabs="dockedTabs" :tab="activeTab" :config="config" :server-config="serverConfig" :ssh-config="sshConfig" :ssh-status="sshStatus" :items="items" :client-running="clientRunning" :server-running="serverRunning" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:tab="activeTab = $event" @update:config="config = $event" @update:server-config="serverConfig = $event" @update:ssh-config="sshConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog" @pick-nic-server="openNicDialog('bindIp')" @pick-public-key="pickPublicKey" @pick-auth-key="pickServerAuthKey" @pick-auth-users="pickServerAuthUsers" @save-custom-length="saveCustomLength" @detach="detachTab" @ssh-connect="sshConnect" @ssh-disconnect="sshDisconnect" @pick-private-key="pickPrivateKey"
      />
      <ConfigPanel
        v-else-if="side === 'client'"
        detached="client" :tab="activeTab" :config="config" :server-config="serverConfig" :ssh-config="sshConfig" :ssh-status="sshStatus" :items="items" :client-running="clientRunning" :server-running="serverRunning" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="config = $event" @update:server-config="serverConfig = $event" @toggle-item="toggleItem" @reset="reset" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog" @pick-public-key="pickPublicKey" @save-custom-length="saveCustomLength"
      />
      <ConfigPanel
        v-else-if="side === 'server'"
        detached="server" :tab="'server'" :config="config" :server-config="serverConfig" :ssh-config="sshConfig" :ssh-status="sshStatus" :items="items" :client-running="clientRunning" :server-running="serverRunning" :local="local" :saved-custom-length="savedTcpLength" :saved-custom-udp-length="savedUdpLength"
        @update:config="config = $event" @update:server-config="serverConfig = $event" @update:ssh-config="sshConfig = $event" @start="start" @stop="requestStop" @start-server="startServer" @stop-server="requestStopServer" @clear="clearLogs" @pick-nic="openNicDialog()" @pick-nic-server="openNicDialog('bindIp')" @pick-public-key="pickPublicKey" @pick-auth-key="pickServerAuthKey" @pick-auth-users="pickServerAuthUsers" @save-custom-length="saveCustomLength" @ssh-connect="sshConnect" @ssh-disconnect="sshDisconnect" @pick-private-key="pickPrivateKey"
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
    <div v-if="errorDialog || infoDialog" class="modal-backdrop" @click.self="errorDialog = null; infoDialog = null"><div class="modal"><button class="modal-close" @click="errorDialog = null; infoDialog = null">×</button><h2>{{ (errorDialog || infoDialog)?.title }}</h2><div class="modal-body"><span :class="['modal-symbol', errorDialog ? 'error' : 'info']">{{ errorDialog ? '×' : 'i' }}</span><p>{{ (errorDialog || infoDialog)?.message }}</p></div><div class="modal-actions"><button v-if="errorDialog?.retry" class="primary" @click="errorDialog = null; start()">{{ t('common.retry') }}</button><button v-else @click="errorDialog = null; infoDialog = null">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="confirmDialog" class="modal-backdrop" @click.self="confirmDialog = null"><div class="modal"><button class="modal-close" @click="confirmDialog = null">×</button><h2>{{ confirmDialog.title }}</h2><div class="modal-body"><span class="modal-symbol info">i</span><p>{{ confirmDialog.message }}</p></div><div class="modal-actions"><button @click="confirmDialog = null">{{ t('common.cancel') }}</button><button class="danger" @click="confirmStop">{{ confirmDialog.action === 'exit' ? t('confirm.exitButton') : t('common.confirm') }}</button></div></div></div>
    <div v-if="settingsDialog" class="modal-backdrop" @click.self="settingsDialog = false"><div class="modal"><button class="modal-close" @click="settingsDialog = false">×</button><h2>{{ t('settings.title') }}</h2><div class="settings-form"><label><span>{{ t('settings.language') }}</span><select :value="locale" @change="onLocaleChange($event)"><option value="en">English</option><option value="zh">中文</option></select></label><label><span>{{ t('settings.theme') }}</span><select :value="theme" @change="onThemeChange($event)"><option value="light">{{ t('settings.light') }}</option><option value="dark">{{ t('settings.dark') }}</option></select></label></div><p class="settings-note">{{ t('settings.engineNote') }}</p><div class="modal-actions"><button class="primary" @click="settingsDialog = false">{{ t('common.confirm') }}</button></div></div></div>
    <div v-if="nicDialog" class="modal-backdrop"><div class="modal nic-modal"><h2>{{ t('nic.title') }}</h2><p class="nic-hint">{{ t('nic.hint') }}</p><div class="nic-list"><label v-for="(nic, index) in interfaces" :key="nic.ip" class="nic-option"><input type="radio" :value="index" v-model="nicSelected" /><span class="nic-name">{{ nic.interfaceName }}</span><span class="nic-ip">{{ nic.ip }}</span><span class="nic-speed">{{ nic.speedMbps ? nic.speedMbps + ' Mbps' : t('nic.speedUnknown') }}</span></label></div><div class="modal-actions"><button class="primary" @click="nicDialog = false; applyNic(nicSelected, true)">{{ t('nic.confirm') }}</button><button @click="nicDialog = false">{{ t('nic.cancel') }}</button></div></div></div>
    <div v-if="aboutDialog" class="modal-backdrop" @click.self="aboutDialog = false"><div class="modal about-modal"><button class="modal-close" @click="aboutDialog = false">×</button><div class="about-header"><img class="about-logo" :src="appIcon" alt="LinkGauge" /><div><h2>LinkGauge</h2><p class="about-intro">{{ t('about.intro') }}</p></div></div><dl class="about-rows"><div class="about-row"><dt>{{ t('about.version') }}</dt><dd>v{{ appInfo.version }}</dd></div><div class="about-row"><dt>{{ t('about.commit') }}</dt><dd><code>{{ appInfo.commit }}</code></dd></div><div class="about-row"><dt>{{ t('about.author') }}</dt><dd>KISSMonX</dd></div><div class="about-row"><dt>{{ t('about.project') }}</dt><dd><button class="about-link" :title="APP_PROJECT_URL" @click="openLink(APP_PROJECT_URL)">github</button></dd></div></dl><div class="about-actions"><button class="primary" @click="openLink(APP_ISSUES_URL)">{{ t('about.feedback') }}</button></div><p class="about-engine">{{ t('about.engine') }}</p></div></div>
  </div>
</template>
