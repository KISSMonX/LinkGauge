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
  protocol: Protocol
  serverIp: string
  port: number
  duration: number
  parallel: number
  bandwidth: number
  packetLength: number
  interval: number
  iperfPath: string
}

export interface NetworkInfo {
  ip: string
  mac: string
  hostname: string
  interfaceName: string
}

export interface IperfRuntimeInfo {
  available: boolean
  bundled: boolean
  path: string
  version: string
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
