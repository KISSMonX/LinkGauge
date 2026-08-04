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
  /** UDP 报文长度（默认 8KB，最大 64KB） */
  udpPacketLength: number
  interval: number
}

/** 服务端独立配置（与服务端标签页绑定，与客户端参数互不影响） */
export interface ServerConfig {
  port: number
  /** 绑定 IP：留空表示绑定所有网卡（默认 0.0.0.0 双栈） */
  bindIp: string
  /** 日志 / 统计信息输出间隔（秒） */
  interval: number
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
  savedTcpLength: number
  savedUdpLength: number
  /** 汇总数据（由驱动窗口维护 startedAt/completed/total，指标类字段各窗口本地推导） */
  summary: TestSummary
  /** 界面语言（默认英文） */
  locale: 'zh' | 'en'
  /** 主题外观（默认亮色） */
  theme: 'light' | 'dark'
}

/** 子窗口关闭（或点击「停靠回主窗口」）时通知主窗口把标签收回 */
export interface DockEvent {
  side: 'client' | 'server'
}
