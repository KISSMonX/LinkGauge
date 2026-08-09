/**
 * In-app updater composable: checks GitHub Releases for a newer version, downloads the
 * signed installer in the background, and restarts the app to apply it.
 *
 * The endpoint and the minisign public key live in `src-tauri/tauri.conf.json`
 * (`plugins.updater`); the matching private key only ever exists as a CI secret.
 */
import { ref, type Ref } from 'vue'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import type { LogEntry } from '../types'

/** 更新流程状态：空闲 / 查询中 / 下载中 / 已下载待重启 */
export type UpdateStage = 'idle' | 'checking' | 'downloading' | 'ready'

export function useUpdater(
  isTauri: () => boolean,
  log: (level: LogEntry['level'], message: string, module?: string) => void,
  errorDialog: Ref<{ title: string; message: string } | null>,
  infoDialog: Ref<{ title: string; message: string } | null>,
  t: (key: import('../i18n').MessageKey, vars?: Record<string, string | number>) => string,
  currentVersion: () => string,
) {
  const updateStage = ref<UpdateStage>('idle')
  /** 待安装的新版本号（不含 v 前缀） */
  const updateVersion = ref('')
  /** 下载进度百分比；服务端未返回 Content-Length 时保持 0，界面退化为无百分比提示 */
  const updateProgress = ref(0)
  /** 「已下载，重启生效」弹窗 */
  const updateReadyDialog = ref(false)
  /** 检查后没有新版本时的一次性提示（仅手动检查显示） */
  const updateUpToDate = ref(false)

  // 已下载但尚未安装的更新句柄。重启弹窗被「稍后」关掉后仍然保留，
  // 用户再次点击「检查更新」时直接复用，不重复下载。
  let pending: Update | null = null

  /**
   * @param silent 启动时的静默检查：无新版本或失败都只写日志，不弹窗打断用户。
   */
  async function checkForUpdate(silent = false) {
    // 查询 / 下载进行中时忽略重复触发；'ready' 不在此列，交给下面的 pending 分支
    if (updateStage.value === 'checking' || updateStage.value === 'downloading') return
    if (!isTauri()) {
      if (!silent) infoDialog.value = { title: t('preview.title'), message: t('preview.update') }
      return
    }
    // 已经下载好的更新不必再查一次，直接把重启弹窗拉回来
    if (pending) { updateReadyDialog.value = true; return }

    updateUpToDate.value = false
    updateStage.value = 'checking'
    if (!silent) log('INFO', t('update.log.checking'))
    let found: Update | null = null
    try {
      found = await check({ timeout: 30000 })
    } catch (e) {
      updateStage.value = 'idle'
      log(silent ? 'WARN' : 'ERROR', t('update.checkFailed', { reason: String(e) }))
      if (!silent) errorDialog.value = { title: t('update.failedTitle'), message: t('update.checkFailed', { reason: String(e) }) }
      return
    }
    if (!found) {
      updateStage.value = 'idle'
      log('INFO', t('update.log.latest', { version: currentVersion() }))
      if (!silent) updateUpToDate.value = true
      return
    }

    updateVersion.value = found.version
    updateProgress.value = 0
    updateStage.value = 'downloading'
    log('INFO', t('update.log.found', { version: found.version, current: found.currentVersion }))
    let downloaded = 0
    let total = 0
    try {
      await found.download((event) => {
        if (event.event === 'Started') total = event.data.contentLength ?? 0
        else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength
          if (total > 0) updateProgress.value = Math.min(100, Math.round((downloaded / total) * 100))
        } else if (event.event === 'Finished') updateProgress.value = 100
      })
    } catch (e) {
      updateStage.value = 'idle'
      updateVersion.value = ''
      // Update 是后端资源句柄，下载失败后不再持有，必须显式释放
      try { await found.close() } catch { /* 释放失败无补救手段，忽略 */ }
      log('ERROR', t('update.downloadFailed', { reason: String(e) }))
      if (!silent) errorDialog.value = { title: t('update.failedTitle'), message: t('update.downloadFailed', { reason: String(e) }) }
      return
    }
    pending = found
    updateStage.value = 'ready'
    updateReadyDialog.value = true
    log('INFO', t('update.log.downloaded', { version: found.version }))
  }

  /** 安装已下载的更新并重启。Windows 上 NSIS 安装器会自行结束本进程并拉起新版本，
   *  install() 之后的 relaunch() 只对 macOS / Linux 生效，因此它抛错不算失败。 */
  async function restartToUpdate() {
    if (!pending) return
    updateReadyDialog.value = false
    try {
      await pending.install()
    } catch (e) {
      updateStage.value = 'ready'
      updateReadyDialog.value = true
      log('ERROR', t('update.installFailed', { reason: String(e) }))
      errorDialog.value = { title: t('update.failedTitle'), message: t('update.installFailed', { reason: String(e) }) }
      return
    }
    try { await relaunch() } catch (e) { log('WARN', String(e)) }
  }

  return {
    updateStage,
    updateVersion,
    updateProgress,
    updateReadyDialog,
    updateUpToDate,
    checkForUpdate,
    restartToUpdate,
  }
}
