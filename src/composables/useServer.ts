/**
 * Server lifecycle composable: manages riperf3 server start, stop, state recovery,
 * and overview statistics.
 *
 * Extracted from App.vue (~60 lines), encapsulates all server state and operations.
 * State refs are returned for App.vue to use in syncBundle / applySync / handleEvent / templates.
 */
import { ref, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import type { LogEntry, MetricPoint, ServerConfig, ServerRuntimeStatus } from '../types'

export function useServer(
  isTauri: () => boolean,
  log: (level: LogEntry['level'], message: string, module?: string) => void,
  errorDialog: Ref<{ title: string; message: string } | null>,
  t: (...args: any[]) => string,
  locale: Ref<'zh' | 'en'>,
  serverConfig: Ref<ServerConfig>,
  local: Ref<{ ip: string; speedMbps: number }>,
) {
  // ---- Server state ----
  const serverRunning = ref(false)
  const serverSession = ref('')
  const serverUptime = ref(0)
  const serverCompleted = ref(0)
  const serverServing = ref(false)
  const serverPoints = ref<MetricPoint[]>([])
  const serverPeerIp = ref('')
  const serverPeerPort = ref(0)

  // ---- Server operations ----

  async function refreshServerState(announce = false) {
    if (!isTauri()) return serverRunning.value
    let status: ServerRuntimeStatus | null = null
    try {
      status = await invoke<ServerRuntimeStatus | null>('get_server_status')
    } catch {
      return false
    }
    if (!status) {
      serverRunning.value = false
      serverSession.value = ''
      return false
    }
    const recovered = !serverRunning.value || serverSession.value !== status.sessionId
    serverSession.value = status.sessionId
    serverRunning.value = true
    serverConfig.value = { ...serverConfig.value, bindIp: status.bindIp, port: status.port, interval: status.interval }
    if (announce && recovered) log('INFO', t('log.serverRecovered', { addr: status.bindIp || t('sdash.allAdapters'), port: status.port }))
    return true
  }

  async function startServer() {
    const cfg = serverConfig.value
    if (serverRunning.value) return
    if (isTauri()) {
      try { if (await refreshServerState(true)) return } catch (e) { log('WARN', String(e)) }
    }
    if (cfg.port < 1 || cfg.port > 65535) { errorDialog.value = { title: t('err.paramError'), message: t('err.port') }; return }
    if (cfg.interval < 1 || cfg.interval > 60) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverInterval') }; return }
    if (cfg.idleTimeout > 86400 || cfg.maxDuration > 86400) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverLimitRange') }; return }
    if (cfg.bitrateLimit > 1000000) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverRateRange') }; return }
    if (cfg.authEnabled && (!cfg.authPrivateKeyPath.trim() || !cfg.authUsersPath.trim())) { errorDialog.value = { title: t('err.paramError'), message: t('err.serverAuthIncomplete') }; return }
    const bindTarget = cfg.bindIp.trim() || local.value.ip
    log('INFO', t('log.startServer', { addr: cfg.bindIp.trim() ? cfg.bindIp : t('sdash.allAdapters'), port: cfg.port }))
    if (!isTauri()) { serverRunning.value = true; log('INFO', t('log.previewServer')); return }
    serverUptime.value = 0; serverCompleted.value = 0; serverServing.value = false; serverPoints.value = []
    serverPeerIp.value = ''; serverPeerPort.value = 0
    try {
      serverSession.value = await invoke<string>('start_test', {
        request: {
          taskId: 'server', mode: 'server', protocol: 'tcp', transferMode: 'time',
          serverIp: local.value.ip, localIp: local.value.ip, bindIp: cfg.bindIp,
          locale: locale.value, port: cfg.port, duration: 0, parallel: 0, bandwidth: 0,
          packetLength: 0, interval: cfg.interval,
          serverAuthEnabled: cfg.authEnabled,
          serverAuthPrivateKeyPath: cfg.authEnabled ? cfg.authPrivateKeyPath.trim() : '',
          serverAuthUsersPath: cfg.authEnabled ? cfg.authUsersPath.trim() : '',
          serverAuthPkcs1Padding: cfg.authPkcs1Padding,
          serverIdleTimeout: cfg.idleTimeout,
          serverMaxDuration: cfg.maxDuration,
          serverBitrateLimitMbps: cfg.bitrateLimit,
        },
      })
      serverRunning.value = true
    } catch (error) {
      try { if (await refreshServerState(true)) return } catch { /* keep original error */ }
      log('ERROR', String(error))
      errorDialog.value = { title: t('err.serverStartFailed'), message: String(error) }
    }
  }

  async function stopServer() {
    if (!serverRunning.value) return
    try {
      if (isTauri() && serverSession.value) await invoke('stop_test', { sessionId: serverSession.value })
      serverRunning.value = false; serverSession.value = ''
      log('INFO', t('log.stopServer'))
    } catch (e) {
      log('WARN', String(e))
      errorDialog.value = { title: t('err.serverError'), message: String(e) }
    }
  }

  return {
    serverRunning,
    serverSession,
    serverUptime,
    serverCompleted,
    serverServing,
    serverPoints,
    serverPeerIp,
    serverPeerPort,
    refreshServerState,
    startServer,
    stopServer,
  }
}
