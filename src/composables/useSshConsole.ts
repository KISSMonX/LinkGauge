/**
 * SSH remote console composable: manages SSH connections, terminal output,
 * event handling, and session state.
 *
 * Extracted from App.vue (~120 lines), encapsulates all SSH state and operations.
 * State refs are returned for App.vue to use in syncBundle / applySync / template bindings.
 */
import { reactive, ref, type Ref } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { open } from '@tauri-apps/plugin-dialog'
import { clearTerminal, createTerminal, writeTerminal } from '../terminal'
import type { LogEntry, SshConfig, SshEvent, SshStatus } from '../types'

export function useSshConsole(
  isTauri: () => boolean,
  log: (level: LogEntry['level'], message: string, module?: string) => void,
  errorDialog: Ref<{ title: string; message: string } | null>,
  infoDialog: Ref<{ title: string; message: string } | null>,
  t: (...args: any[]) => string,
  sshConfig: Ref<SshConfig>,
  serverView: Ref<string>,
) {
  // ---- SSH state ----
  const sshSession = ref('')
  const sshStatus = ref<SshStatus>('idle')
  const sshTerminal = reactive(createTerminal())
  const sshPrimed = ref(0)
  let sshPending: SshEvent[] | null = null
  let sshCols = 120
  let sshRows = 24

  // ---- SSH operations ----

  async function pickPrivateKey() {
    if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.pickKey') }; return }
    try {
      const path = await open({ title: t('ssh.keyPick'), multiple: false, directory: false })
      if (typeof path === 'string') {
        sshConfig.value = { ...sshConfig.value, privateKeyPath: path }
        log('INFO', t('ssh.log.keyPicked', { path }), 'ssh')
      }
    } catch (e) { errorDialog.value = { title: t('err.openDirFailed'), message: String(e) } }
  }

  async function sshConnect() {
    if (sshStatus.value !== 'idle') return
    if (!isTauri()) { infoDialog.value = { title: t('preview.title'), message: t('preview.ssh') }; return }
    clearTerminal(sshTerminal)
    sshPrimed.value = 0
    serverView.value = 'ssh'
    sshStatus.value = 'connecting'
    try {
      sshSession.value = await invoke<string>('ssh_connect', {
        request: { ...sshConfig.value, cols: sshCols, rows: sshRows },
      })
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

  async function sshSend(data: string) {
    if (!sshSession.value || !isTauri()) return
    try { await invoke('ssh_send', { sessionId: sshSession.value, data }) } catch (e) { log('WARN', String(e), 'ssh') }
  }

  function sshResize(cols: number, rows: number) {
    if (cols === sshCols && rows === sshRows) return
    sshCols = cols; sshRows = rows
    if (sshSession.value && isTauri()) invoke('ssh_resize', { sessionId: sshSession.value, cols, rows }).catch(() => {})
  }

  async function primeConsole(sessionId: string) {
    if (!isTauri()) return
    sshPending = []
    try {
      const snapshot = await invoke<{ text: string; endOffset: number; connected: boolean }>(
        'ssh_scrollback', { sessionId },
      )
      if (sshSession.value !== sessionId) return
      clearTerminal(sshTerminal)
      writeTerminal(sshTerminal, snapshot.text)
      sshPrimed.value = snapshot.endOffset
      for (const event of sshPending) {
        if (event.sessionId === sessionId && (event.offset ?? 0) >= snapshot.endOffset) {
          writeTerminal(sshTerminal, event.message || '')
        }
      }
      if (snapshot.connected) sshStatus.value = 'connected'
    } catch {
      if (sshSession.value === sessionId) { sshSession.value = ''; sshStatus.value = 'idle' }
    } finally { sshPending = null }
  }

  function handleSshEvent(event: SshEvent) {
    if (!sshSession.value || event.sessionId !== sshSession.value) return
    if (event.type === 'data') {
      if (sshPending) { sshPending.push(event); return }
      if ((event.offset ?? 0) < sshPrimed.value) return
      writeTerminal(sshTerminal, event.message || '')
      return
    }
    if (event.type === 'status') {
      if (event.message === 'connected' || event.message === 'connecting') sshStatus.value = event.message
      return
    }
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

  return {
    sshSession,
    sshStatus,
    sshTerminal,
    sshPrimed,
    pickPrivateKey,
    sshConnect,
    sshDisconnect,
    sshSend,
    sshResize,
    primeConsole,
    handleSshEvent,
  }
}
