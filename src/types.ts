export type AppMode = 'server' | 'client'
export type Protocol = 'tcp' | 'udp'
export type TaskStatus = 'waiting' | 'running' | 'success' | 'failed' | 'stopped'
export type LogLevel = 'INFO' | 'WARN' | 'ERROR'

export interface TestItem {
  id: string
  label: string
  protocol: Protocol | 'ping'
  enabled: boolean
  status: TaskStatus
}

export interface TestConfig {
  mode: AppMode
  serverIp: string
  port: number
  duration: number
  parallel: number
  bandwidth: number
  /** TCP 报文长度（默认 128KB，最大 1MB） */
  packetLength: number
  /** UDP 报文长度（默认 1460 B，与 iperf3 的 DEFAULT_UDP_BLKSIZE 一致；最大 64KB）。
   *  超过路径 MTU 会触发 IP 分片，丢包率将被分片放大而无法与原生 iperf3 结果对比 */
  udpPacketLength: number
  interval: number
  /** 预热秒数（0 = 不预热，对应 iperf3 `-O`）：跳过前 N 秒统计，须小于 duration */
  omitSecs: number
  /** TCP 套接字缓冲区（KB，0 = 自动，对应 iperf3 `-w`） */
  windowKb: number
  /** 客户端数据流源端口（0 = 自动，对应 iperf3 `--cport`） */
  cport: number
  /** IP 协议族（0 = 自动，4 = 仅 IPv4，6 = 仅 IPv6） */
  ipVersion: number
  /** 测试结束后拉取服务端视角的输出（--get-server-output） */
  getServerOutput: boolean
  /** 测试结束条件：time = 按时长，bytes = 按传输字节数（-n），blocks = 按块数（-k） */
  transferMode: 'time' | 'bytes' | 'blocks'
  /** 按量测试的数量：bytes 模式为 MB，blocks 模式为块数 */
  transferAmount: number
  /** DSCP 值（0 = 不设置，1–63，对应 --dscp） */
  dscp: number
  /** TCP 拥塞控制算法（空 = 默认，对应 -C；仅 Linux/FreeBSD 生效） */
  congestionAlgo: string
  /** UDP 数据报禁止分片（--dont-fragment；仅 IPv4） */
  udpDontFragment: boolean
  /** 使用 MPTCP 多路径 TCP（-m/--multipath；需两端内核支持） */
  mptcp: boolean
  /** 启用 iperf3 认证（对端以 --rsa-private-key-path + --authorized-users-path 启动时必需） */
  authEnabled: boolean
  authUsername: string
  /** 认证密码：只在内存与窗口间同步，不写入 localStorage、不随配置导出 */
  authPassword: string
  /** 服务端 RSA 公钥文件路径，用于加密凭据 */
  authPublicKeyPath: string
  /** 对 iperf3 < 3.17 的服务端改用 PKCS#1 v1.5 填充（3.17+ 默认 OAEP） */
  authPkcs1Padding: boolean
}

/** 服务端独立配置（与服务端标签页绑定，与客户端参数互不影响） */
export interface ServerConfig {
  port: number
  /** 绑定 IP：留空表示绑定所有网卡（默认 0.0.0.0 双栈） */
  bindIp: string
  /** 日志 / 统计信息输出间隔（秒） */
  interval: number
  /** 启用服务端 iperf3 认证（要求客户端提供用户名/密码凭据） */
  authEnabled: boolean
  /** RSA 私钥文件路径（PEM，用于解密客户端凭据，--rsa-private-key-path） */
  authPrivateKeyPath: string
  /** 授权用户文件路径（--authorized-users-path；每行 `用户名,sha256hex`，见 riperf3 auth.rs） */
  authUsersPath: string
  /** 对 iperf3 < 3.17 的客户端改用 PKCS#1 v1.5 填充（3.17+ 默认 OAEP） */
  authPkcs1Padding: boolean
  /** 空闲超时（秒，0 = 不限制）：N 秒无客户端连接则自动停止服务 */
  idleTimeout: number
  /** 单次测试最大时长（秒，0 = 不限制），超限测试在参数交换阶段被拒绝 */
  maxDuration: number
  /** 服务端带宽上限（Mbps，0 = 不限制），聚合吞吐超限的测试被终止 */
  bitrateLimit: number
}

/** 服务端页的 SSH 远程控制台连接参数（密码与私钥口令不落盘，仅在内存与窗口间流转） */
export interface SshConfig {
  host: string
  port: number
  username: string
  /** 认证方式：密码 / 私钥 */
  authMethod: 'password' | 'key'
  password: string
  privateKeyPath: string
  passphrase: string
}

/** SSH 会话状态：未连接 / 连接中 / 已连接 */
export type SshStatus = 'idle' | 'connecting' | 'connected'

/** 后端 `ssh-event` 事件（远端输出、连接状态与生命周期日志） */
export interface SshEvent {
  sessionId: string
  type: 'status' | 'log' | 'data' | 'closed' | 'error'
  level?: LogLevel
  message?: string
  /** data 事件：本块文本在会话输出流中的起始偏移，用于与回放缓冲去重 */
  offset?: number
}

/** 会话快照：回放缓冲 + 当前连接状态。
 *  状态一并返回是必要的——同机连接时远端 shell 可能在 ssh_connect 返回会话 id 之前就已就绪，
 *  那一刻广播的 connected 事件会因前端还不知道会话 id 而被丢弃 */
export interface SshSnapshot {
  text: string
  endOffset: number
  connected: boolean
}

export interface NetworkInfo {
  ip: string
  mac: string
  hostname: string
  interfaceName: string
  speedMbps: number
}

export interface InterfaceInfo {
  ip: string
  mac: string
  interfaceName: string
  speedMbps: number
}

export interface MetricPoint {
  second: number
  bandwidthMbps: number
  transferMb: number
  jitterMs: number
  lossPercent: number
  retransmits: number
}

export interface LogEntry {
  time: string
  level: LogLevel
  module: string
  message: string
}

export interface BackendEvent {
  sessionId: string
  taskId: string
  type: 'status' | 'log' | 'metric' | 'error' | 'complete'
  status?: TaskStatus
  level?: LogLevel
  message?: string
  metric?: MetricPoint
  logPath?: string
  /** 环境性失败（服务端不可达等）：队列中剩余引擎项必然同样失败，前端据此中止整个队列 */
  fatal?: boolean
}

export interface TestSummary {
  startedAt: string
  completed: number
  total: number
  averageBandwidth: number
  maxBandwidth: number
  minBandwidth: number
  totalTransferMb: number
  pingAverage: number
  lossPercent: number
  jitterMs: number
  logPaths: string[]
}

/** 单个测试项目完成后的历史数据缓存（仅内存，退出应用即销毁），key 为测试项目 id */
export interface ItemHistory {
  status: TestItem['status']
  points: MetricPoint[]
}

/** 多窗口（主窗口 + 分离的客户端/服务端窗口）间同步的完整状态包 */
export interface SyncState {
  config: TestConfig
  serverConfig: ServerConfig
  items: TestItem[]
  local: NetworkInfo
  clientRunning: boolean
  serverRunning: boolean
  clientSession: string
  serverSession: string
  /** 执行队列及其游标（由驱动窗口维护） */
  queue: string[]
  queueIndex: number
  /** 驱动客户端队列的窗口 label（main / client），其他窗口只展示不启动下一项 */
  driver: string
  /** 发出本同步包的窗口 label：队列推进状态只采纳驱动窗口（source === driver）的同步 */
  source: string
  savedTcpLength: number
  savedUdpLength: number
  /** 汇总数据（由驱动窗口维护 startedAt/completed/total，指标类字段各窗口本地推导） */
  summary: TestSummary
  /** 界面语言（默认英文） */
  locale: 'zh' | 'en'
  /** 主题外观（默认亮色） */
  theme: 'light' | 'dark'
  // —— 测试运行中的瞬时数据（测试进行中拖出标签时，新窗口据此恢复完整界面）——
  /** 客户端实时曲线（各窗口从同一批事件维护同一份数据，整数组替换防止重复） */
  points: MetricPoint[]
  /** 最近一次已完成测试的图表数据缓存 */
  completedPoints: MetricPoint[]
  completedLabel: string
  /** 各测试项目的历史数据缓存（key = 项目 id；查看历史曲线时展示） */
  itemHistory: Record<string, ItemHistory>
  /** 当前查看的历史项目 id（'' = 未选择，显示最近一次完成项） */
  selectedHistoryId: string
  /** 客户端任务队列总体进度（0-100，仅驱动窗口修改） */
  progress: number
  /** 本轮测试开始时间戳（ms），各窗口据此推算已用时，避免逐秒同步 elapsed */
  startedAt: number
  connected: boolean
  /** 本次连接的客户端本地端口（控制连接建立时由后端广播） */
  clientLocalPort: number
  /** 引擎日志（不含各窗口自己的 UI 日志；UI 日志本地保留，避免跨窗口污染） */
  logs: LogEntry[]
  // 服务端独立概览与曲线（心跳事件携带绝对值，各窗口维护同一份）
  serverUptime: number
  serverCompleted: number
  serverServing: boolean
  serverPoints: MetricPoint[]
  serverPeerIp: string
  serverPeerPort: number
  // SSH 远程控制台：连接参数与会话状态跨窗口同步；
  // 控制台正文不进同步包（输出量大），各窗口自行监听 ssh-event 并按需拉取回放缓冲
  sshConfig: SshConfig
  sshSession: string
  sshStatus: SshStatus
}

/** 子窗口关闭（或点击「停靠回主窗口」）时通知主窗口把标签收回 */
export interface DockEvent {
  side: 'client' | 'server'
}
