/**
 * In-app updater composable: checks GitHub Releases for a newer version, downloads the
 * signed package on demand, and restarts the app to apply it.
 *
 * The endpoint and the minisign public key live in `src-tauri/tauri.conf.json`
 * (`plugins.updater`); the matching private key only ever exists as a CI secret.
 *
 * State refs are returned for App.vue to use in its template; the cross-window
 * `update-state` event is emitted here but listened to in App.vue, matching how the
 * other composables leave `listen()` registration to the owner.
 */
import { computed, ref, type Ref } from 'vue'
import { check, type Update } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { emit } from '@tauri-apps/api/event'
import { invoke } from '@tauri-apps/api/core'
import type { LogEntry } from '../types'

/** 更新流程状态：空闲 / 查询中 / 发现新版本待下载 / 下载中 / 已下载待重启 */
export type UpdateStage = 'idle' | 'checking' | 'available' | 'downloading' | 'ready'

/**
 * 跨窗口广播的更新阶段。刻意不含下载进度：进度每收到一个数据块就变一次，
 * 广播它会把事件通道淹掉（同 SyncState 里不放高频数据的理由）。
 */
export interface UpdateStateEvent { instance: string; stage: UpdateStage; version: string }

export function useUpdater(
  isTauri: () => boolean,
  log: (level: LogEntry['level'], message: string, module?: string) => void,
  errorDialog: Ref<{ title: string; message: string } | null>,
  infoDialog: Ref<{ title: string; message: string } | null>,
  t: (key: import('../i18n').MessageKey, vars?: Record<string, string | number>) => string,
  currentVersion: () => string,
  /** 安装前的收尾：停掉正在跑的客户端队列 / 服务端，避免装完重启后留下半截状态 */
  beforeInstall: () => Promise<void>,
) {
  const updateStage = ref<UpdateStage>('idle')
  /** 待下载 / 待安装的新版本号（不含 v 前缀） */
  const updateVersion = ref('')
  /** 下载进度百分比；服务端未返回 Content-Length 时保持 0，界面退化为无百分比提示 */
  const updateProgress = ref(0)
  /** 「已下载，重启生效」弹窗 */
  const updateReadyDialog = ref(false)
  /** 检查后没有新版本时的一次性提示（仅手动检查显示） */
  const updateUpToDate = ref(false)
  /**
   * 本次安装形态是否支持自更新。Windows / macOS 恒为 true；Linux 只有 AppImage
   * 能自替换，deb / rpm 装出来的进程拿不到 APPIMAGE 环境变量，只能引导用户去
   * Releases 页面手动下载——那就干脆不下这个装不上的包。
   */
  const updateSelfUpdatable = ref(true)

  /** 本窗口实例 id：广播会回送给自己，靠它过滤 */
  const instance = `updater-${Date.now()}-${Math.random().toString(36).slice(2)}`
  /**
   * 正在查询 / 下载的其它窗口实例。用集合而不是布尔量：三个窗口时，其中一个报
   * 「空闲」不能把另一个仍在下载的窗口的忙标记抹掉。
   *
   * 忙窗口在自己关闭前不会广播「空闲」，理论上会留下一条永不解除的忙记录；实际
   * 触发不到——「关于」页只存在于主窗口（`side === 'hub'`），而主窗口关闭即退出应用。
   */
  const remoteBusy = ref(new Set<string>())

  // 已查到但尚未下载（stage='available'）或已下载待安装（stage='ready'）的更新句柄。
  // 「稍后」关掉重启弹窗后仍然保留，用户再点按钮时直接复用，不重复下载。
  let handle: Update | null = null

  /** 阶段变化时广播给其它窗口；进度不广播（见 UpdateStateEvent 注释） */
  function broadcast() {
    if (!isTauri()) return
    const payload: UpdateStateEvent = { instance, stage: updateStage.value, version: updateVersion.value }
    void emit('update-state', payload).catch(() => { /* 广播失败只影响跨窗口互斥，不影响本窗口流程 */ })
  }

  function setStage(stage: UpdateStage) {
    updateStage.value = stage
    broadcast()
  }

  /** App.vue 的 `update-state` 监听回调：记录哪些窗口正忙 */
  function handleUpdateState(payload: UpdateStateEvent) {
    if (payload.instance === instance) return
    const busyNow = payload.stage === 'checking' || payload.stage === 'downloading'
    // Set 原地增删不触发 Vue 的依赖收集，换一个新实例赋值
    const next = new Set(remoteBusy.value)
    if (busyNow) next.add(payload.instance)
    else next.delete(payload.instance)
    remoteBusy.value = next
  }

  /** 启动时查询本次安装形态能否自更新（Linux 上区分 AppImage 与 deb / rpm） */
  async function primeUpdater() {
    if (!isTauri()) return
    try {
      updateSelfUpdatable.value = await invoke<boolean>('updater_supported')
    } catch (e) {
      // 查询失败按“支持”处理：真装不上时 install() 会报错，届时再引导手动下载
      log('WARN', String(e))
    }
  }

  /** 查询 / 下载进行中（本窗口或其它窗口），此时禁止再次发起 */
  const updateBusy = computed(() =>
    remoteBusy.value.size > 0 || updateStage.value === 'checking' || updateStage.value === 'downloading')

  /**
   * 查询是否有新版本。**只查不下载**：下载要花掉几十 MB 流量，交给用户在「关于」
   * 页显式点击，启动时的静默检查只负责把红点亮起来。
   *
   * @param silent 启动时的静默检查：无新版本或失败都只写日志，不弹窗打断用户。
   */
  async function checkForUpdate(silent = false) {
    if (updateBusy.value) return
    if (!isTauri()) {
      if (!silent) infoDialog.value = { title: t('preview.title'), message: t('preview.update') }
      return
    }
    // 已经查到 / 下载好的更新不必再查一次，界面此时展示的是「下载」或「重启」入口
    if (handle) { if (updateStage.value === 'ready') updateReadyDialog.value = true; return }

    updateUpToDate.value = false
    setStage('checking')
    if (!silent) log('INFO', t('update.log.checking'))
    let found: Update | null = null
    try {
      found = await check({ timeout: 30000 })
    } catch (e) {
      setStage('idle')
      log(silent ? 'WARN' : 'ERROR', t('update.checkFailed', { reason: String(e) }))
      if (!silent) errorDialog.value = { title: t('update.failedTitle'), message: t('update.checkFailed', { reason: String(e) }) }
      return
    }
    if (!found) {
      setStage('idle')
      log('INFO', t('update.log.latest', { version: currentVersion() }))
      if (!silent) updateUpToDate.value = true
      return
    }

    handle = found
    updateVersion.value = found.version
    updateProgress.value = 0
    setStage('available')
    log('INFO', t('update.log.found', { version: found.version, current: found.currentVersion }))
  }

  /** 下载已查到的新版本。由用户在「关于」页点击触发，下载完成后弹重启确认。 */
  async function downloadUpdate() {
    if (updateBusy.value || !handle || updateStage.value !== 'available') return
    // deb / rpm 装不了下载来的包，不浪费用户流量；界面在这个分支给的是「前往下载页面」
    if (!updateSelfUpdatable.value) return
    const target = handle
    updateProgress.value = 0
    setStage('downloading')
    log('INFO', t('update.log.downloading', { version: target.version }))
    let downloaded = 0
    let total = 0
    try {
      await target.download((event) => {
        if (event.event === 'Started') total = event.data.contentLength ?? 0
        else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength
          if (total > 0) updateProgress.value = Math.min(100, Math.round((downloaded / total) * 100))
        } else if (event.event === 'Finished') updateProgress.value = 100
      })
    } catch (e) {
      // Update 是后端资源句柄，下载失败后不再持有，必须显式释放
      handle = null
      updateVersion.value = ''
      setStage('idle')
      try { await target.close() } catch { /* 释放失败无补救手段，忽略 */ }
      log('ERROR', t('update.downloadFailed', { reason: String(e) }))
      errorDialog.value = { title: t('update.failedTitle'), message: t('update.downloadFailed', { reason: String(e) }) }
      return
    }
    setStage('ready')
    updateReadyDialog.value = true
    log('INFO', t('update.log.downloaded', { version: target.version }))
  }

  /** 安装已下载的更新并重启。Windows 上 NSIS 安装器会自行结束本进程并拉起新版本，
   *  install() 之后的 relaunch() 只对 macOS / Linux 生效，因此它抛错不算失败。 */
  async function restartToUpdate() {
    if (!handle || updateStage.value !== 'ready') return
    const target = handle
    // 先补查一次能否自更新：primeUpdater 查询失败时会乐观地按「支持」处理，而下面
    // beforeInstall 会停掉用户正在跑的测试——不能为一个注定失败的 install() 白停
    await primeUpdater()
    if (!updateSelfUpdatable.value) {
      updateReadyDialog.value = false
      // 退回 available：「关于」页的按钮随之变成「前往下载」，把用户导向手动升级
      setStage('available')
      errorDialog.value = { title: t('update.failedTitle'), message: t('update.manualOnlyMessage') }
      return
    }
    updateReadyDialog.value = false
    // 安装器会直接终止本进程：先把跑着的队列 / 服务端收干净，让后端有机会写完日志、
    // 释放监听端口，而不是被杀在半路
    try { await beforeInstall() } catch (e) { log('WARN', String(e)) }
    try {
      await target.install()
    } catch (e) {
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
    updateSelfUpdatable,
    updateBusy,
    primeUpdater,
    handleUpdateState,
    checkForUpdate,
    downloadUpdate,
    restartToUpdate,
  }
}
