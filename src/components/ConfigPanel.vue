<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import type { NetworkInfo, ServerConfig, SshConfig, SshStatus, TestConfig, TestItem } from '../types'
import { useI18n, type MessageKey } from '../i18n'
import Icon from './Icon.vue'

/** tabs：主窗口已停靠的标签列表（undefined = 分离窗口，仅显示 detached 一侧） */
const props = defineProps<{ tabs?: ('client' | 'server')[]; detached?: 'client' | 'server'; tab: 'client' | 'server'; config: TestConfig; serverConfig: ServerConfig; sshConfig: SshConfig; sshStatus: SshStatus; items: TestItem[]; clientRunning: boolean; serverRunning: boolean; local: NetworkInfo; savedCustomLength: number; savedCustomUdpLength: number }>()
const emit = defineEmits<{
  'update:tab': [value: 'client' | 'server']
  'update:config': [value: TestConfig]
  'update:server-config': [value: ServerConfig]
  'update:ssh-config': [value: SshConfig]
  /** SSH 远程控制台：连接 / 断开 / 选择私钥文件 */
  'ssh-connect': []
  'ssh-disconnect': []
  'pick-private-key': []
  'toggle-item': [id: string]
  reset: []
  start: []
  stop: []
  'start-server': []
  'stop-server': []
  clear: []
  'pick-nic': []
  'pick-nic-server': []
  /** 选择 iperf3 认证用的服务端 RSA 公钥文件 */
  'pick-public-key': []
  /** 选择服务端认证用的 RSA 私钥文件 */
  'pick-auth-key': []
  /** 选择服务端认证用的授权用户文件 */
  'pick-auth-users': []
  'save-custom-length': [protocol: 'tcp' | 'udp', value: number]
  /** 标签页被拖拽分离为独立窗口 */
  detach: [side: 'client' | 'server']
}>()

const { t } = useI18n()
const itemLabel = (id: string) => t(('cfg.item.' + id) as MessageKey)
const isTauri = () => '__TAURI_INTERNALS__' in window
/** 当前展示的一侧：分离窗口固定为 detached，主窗口跟随激活标签 */
const visibleSide = computed<'client' | 'server'>(() => props.detached ?? props.tab)

// —— 标签页拖拽分离：pointer 事件 + 跟随光标的幽灵标签，拖出阈值松开即分离 ——
const DETACH_THRESHOLD = 100
const drag = ref<{ side: 'client' | 'server'; startX: number; startY: number; ghost: HTMLElement } | null>(null)
const dragFar = ref(false)
const suppressedClick = ref(false)

function startTabDrag(side: 'client' | 'server', event: PointerEvent) {
  // 仅在桌面端、主窗口标签栏可用；分离窗口没有标签栏
  if (!isTauri() || !props.tabs?.length || event.button !== 0) return
  const ghost = document.createElement('div')
  ghost.className = 'tab-ghost'
  ghost.textContent = t(side === 'client' ? 'common.client' : 'common.server')
  document.body.appendChild(ghost)
  drag.value = { side, startX: event.clientX, startY: event.clientY, ghost }
  dragFar.value = false
  ;(event.currentTarget as HTMLElement).setPointerCapture(event.pointerId)
  event.preventDefault()
}
function onTabDragMove(event: PointerEvent) {
  const d = drag.value
  if (!d) return
  const dist = Math.hypot(event.clientX - d.startX, event.clientY - d.startY)
  dragFar.value = dist > DETACH_THRESHOLD
  d.ghost.textContent = t(d.side === 'client' ? 'common.client' : 'common.server')
  d.ghost.classList.toggle('detach', dragFar.value)
  if (dragFar.value) d.ghost.textContent += ` ${t('tab.detachRelease')}`
  d.ghost.style.transform = `translate(${event.clientX - 8}px, ${event.clientY - 8}px)`
}
function endTabDrag(event: PointerEvent) {
  const d = drag.value
  if (!d) return
  drag.value = null
  dragFar.value = false
  d.ghost.remove()
  // pointercancel（捕获中断/窗口失焦）只清理，不触发分离
  if (event.type !== 'pointerup') return
  // 拖出阈值 = 分离；阈值内松开 = 弹回（本次拖拽抑制随后的 click 切标签）
  if (Math.hypot(event.clientX - d.startX, event.clientY - d.startY) > DETACH_THRESHOLD) {
    suppressedClick.value = true
    emit('detach', d.side)
  }
}
function onTabClick() {
  // 拖拽结束后浏览器仍会派发 click，这里直接吞掉，避免误切换标签
  if (suppressedClick.value) { suppressedClick.value = false; return }
}
/** 窗口失焦（拖拽中切到其他程序）时清理幽灵标签，避免残留 */
function cancelTabDrag() {
  const d = drag.value
  if (!d) return
  drag.value = null
  dragFar.value = false
  d.ghost.remove()
}
onMounted(() => { window.addEventListener('blur', cancelTabDrag) })
onUnmounted(() => { window.removeEventListener('blur', cancelTabDrag) })

const set = <K extends keyof TestConfig>(key: K, value: TestConfig[K]) => emit('update:config', { ...props.config, [key]: value })
const setServer = <K extends keyof ServerConfig>(key: K, value: ServerConfig[K]) => emit('update:server-config', { ...props.serverConfig, [key]: value })
const setSsh = <K extends keyof SshConfig>(key: K, value: SshConfig[K]) => emit('update:ssh-config', { ...props.sshConfig, [key]: value })
/** SSH 参数在连接建立后锁定，断开后才能修改 */
const sshLocked = computed(() => props.sshStatus !== 'idle')

// —— 悬停/点击提示（测试项介绍 + 参数项注释共用）：
// fixed 定位浮动层不受面板 overflow 裁剪；hover 跟随鼠标，点击固定显示 ——
const tooltip = ref<{ text: string; x: number; y: number; pinned: boolean } | null>(null)
/** 贴近视口右/下缘时回退坐标，避免提示被挤出屏幕 */
const clampTip = (x: number, y: number) => ({ x: Math.min(x + 14, window.innerWidth - 280), y: Math.min(y + 14, window.innerHeight - 70) })
function showTip(event: MouseEvent, text: string) {
  tooltip.value = { text, ...clampTip(event.clientX, event.clientY), pinned: false }
}
function moveTip(event: MouseEvent) {
  if (tooltip.value && !tooltip.value.pinned) tooltip.value = { ...tooltip.value, ...clampTip(event.clientX, event.clientY) }
}
/** 点击切换固定显示：再次点击同一提示关闭；固定期间鼠标移开不隐藏 */
function pinTip(event: MouseEvent, text: string) {
  if (tooltip.value?.pinned && tooltip.value.text === text) {
    tooltip.value = null
    return
  }
  tooltip.value = { text, ...clampTip(event.clientX, event.clientY), pinned: true }
}
function hideTip() {
  if (!tooltip.value?.pinned) tooltip.value = null
}

/** 带宽限制选项：当前网卡速率（默认） + 100 / 1000 / 0（不限制），相同速率去重 */
const bandwidthOptions = computed(() => {
  const nic = props.local.speedMbps > 0 ? props.local.speedMbps : 0
  const options: { value: number; label: string }[] = []
  for (const value of [nic, 100, 1000, 0]) {
    if (options.some((option) => option.value === value)) continue
    const label = value === nic && nic > 0
      ? t('cfg.bandwidthNic', { v: value })
      : value === 0 ? t('cfg.bandwidthUnlimited') : `${value} Mbps`
    options.push({ value, label })
  }
  return options
})

// TCP 报文长度预设（最大 1MB，默认 128KB）
const TCP_PRESETS = [1024, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576]
// DSCP 常用取值（0-63，--dscp；标签为协议常量，无需翻译）
const dscpOptions = [
  { value: 8, label: 'CS1 (8)' }, { value: 16, label: 'CS2 (16)' }, { value: 24, label: 'CS3 (24)' },
  { value: 32, label: 'CS4 (32)' }, { value: 40, label: 'CS5 (40)' }, { value: 48, label: 'CS6 (48)' },
  { value: 56, label: 'CS7 (56)' }, { value: 46, label: 'EF (46)' }, { value: 44, label: 'VA (44)' },
  { value: 10, label: 'AF11 (10)' }, { value: 18, label: 'AF21 (18)' }, { value: 26, label: 'AF31 (26)' },
  { value: 34, label: 'AF41 (34)' }, { value: 1, label: 'LE (1)' }
]
// UDP 报文长度预设（最大 64KB，默认 1460 = iperf3 的 DEFAULT_UDP_BLKSIZE）。
// 1460 与 1472 均不分片（1472 = 1500 MTU 上限），更大的值会触发 IP 分片
const UDP_PRESETS = [128, 512, 1024, 1460, 1472, 4096, 8192, 16384, 32768, 65536]
const formatLength = (value: number) => (value >= 1024 && value % 1024 === 0 ? `${value / 1024} KB` : `${value} bytes`)
const isPreset = (presets: number[], value: number) => presets.includes(value)

/** 报文长度下拉选项：预设值 + 已保存的自定义值 + 「自定义…」 */
function lengthOptions(presets: number[], savedCustom: number) {
  const options = presets.map((value) => ({ value, label: formatLength(value) }))
  if (savedCustom > 0 && !isPreset(presets, savedCustom)) {
    options.push({ value: savedCustom, label: `${formatLength(savedCustom)}（${t('cfg.custom')}）` })
  }
  return options
}
const tcpPacketOptions = computed(() => lengthOptions(TCP_PRESETS, props.savedCustomLength))
const udpPacketOptions = computed(() => lengthOptions(UDP_PRESETS, props.savedCustomUdpLength))

/** select 取值：能匹配选项就用数字，否则为 'custom' */
const selectValue = (options: { value: number }[], current: number) => (options.some((option) => option.value === current) ? current : 'custom')
const tcpPacketSelect = computed(() => selectValue(tcpPacketOptions.value, props.config.packetLength))
const udpPacketSelect = computed(() => selectValue(udpPacketOptions.value, props.config.udpPacketLength))

/** 自定义报文长度弹窗：输入数值 + 选择单位（Bytes / KB / MB），TCP/UDP 上限不同 */
const customDialog = ref(false)
const customTarget = ref<'tcp' | 'udp'>('tcp')
const customValue = ref<number | null>(null)
const customUnit = ref<'bytes' | 'kb' | 'mb'>('kb')
const UNIT_FACTOR = { bytes: 1, kb: 1024, mb: 1024 * 1024 }
const LENGTH_LIMIT = { tcp: 1_048_576, udp: 65_536 }
const customBytes = computed(() => {
  const value = customValue.value
  if (value === null || !Number.isFinite(value) || value <= 0) return 0
  return Math.round(value * UNIT_FACTOR[customUnit.value])
})

function openCustomDialog(target: 'tcp' | 'udp') {
  customTarget.value = target
  // 预填当前报文长度（自定义值优先，否则默认值），按大小换算成最合适的单位
  const length = target === 'tcp'
    ? (props.config.packetLength > 0 && !isPreset(TCP_PRESETS, props.config.packetLength) ? props.config.packetLength : (props.savedCustomLength || 131072))
    : (props.config.udpPacketLength > 0 && !isPreset(UDP_PRESETS, props.config.udpPacketLength) ? props.config.udpPacketLength : (props.savedCustomUdpLength || 1460))
  if (length % (1024 * 1024) === 0) { customValue.value = length / (1024 * 1024); customUnit.value = 'mb' }
  else if (length % 1024 === 0) { customValue.value = length / 1024; customUnit.value = 'kb' }
  else { customValue.value = length; customUnit.value = 'bytes' }
  customDialog.value = true
}
function confirmCustomLength() {
  const bytes = customBytes.value
  const limit = LENGTH_LIMIT[customTarget.value]
  if (bytes < 1 || bytes > limit) return // 超出范围不关闭，用户可继续调整
  customDialog.value = false
  emit('save-custom-length', customTarget.value, bytes)
}

function onPacketChange(target: 'tcp' | 'udp', event: Event) {
  const value = (event.target as HTMLSelectElement).value
  if (value === 'custom') {
    openCustomDialog(target)
  } else {
    if (target === 'tcp') set('packetLength', Number(value))
    else set('udpPacketLength', Number(value))
  }
}
</script>

<template>
  <aside class="panel config-panel">
    <div v-if="tabs" class="mode-tabs">
      <button v-for="side in tabs" :key="side" :class="{ active: tab === side }" :title="t('tab.dragHint')" @pointerdown="startTabDrag(side, $event)" @pointermove="onTabDragMove" @pointerup="endTabDrag" @pointercancel="endTabDrag" @click="onTabClick(); emit('update:tab', side)"><Icon name="monitor" />{{ side === 'client' ? t('common.client') : t('common.server') }}<span class="tab-detach">⇱</span></button>
    </div>
    <template v-if="visibleSide === 'client'">
    <section class="config-section tests-section">
      <div class="section-title"><h2>{{ t('cfg.tests') }}</h2></div>
      <p class="protocol-hint">{{ t('cfg.testsHint') }}</p>
      <div class="test-list">
        <label v-for="(item, index) in items" :key="item.id" @mouseenter="showTip($event, t(('cfg.itemDesc.' + item.id) as MessageKey))" @mousemove="moveTip" @mouseleave="hideTip">
          <input type="checkbox" :checked="item.enabled" :disabled="clientRunning" @change="emit('toggle-item', item.id)" />
          <span>{{ index + 1 }}. {{ itemLabel(item.id) }}</span><span class="drag">≡</span>
        </label>
      </div>
    </section>
    <section class="config-section parameters">
      <div class="section-title"><h2>{{ t('cfg.params') }}</h2><button class="text-button" :disabled="clientRunning" @click="emit('reset')">{{ t('cfg.reset') }}</button></div>
      <label><span>{{ t('cfg.serverIp') }}</span><span class="ip-row"><input :value="config.serverIp" :disabled="clientRunning" @input="set('serverIp', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="clientRunning" :title="t('nic.title')" @click="emit('pick-nic')">{{ t('cfg.nicBtn') }}</button></span></label>
      <label><span>{{ t('cfg.port') }}</span><input type="number" :value="config.port" min="1" max="65535" :disabled="clientRunning" @input="set('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>{{ t('cfg.transferMode') }}</span><select :value="config.transferMode" :disabled="clientRunning" @change="set('transferMode', ($event.target as HTMLSelectElement).value as TestConfig['transferMode'])"><option value="time">{{ t('cfg.transferModeTime') }}</option><option value="bytes">{{ t('cfg.transferModeBytes') }}</option><option value="blocks">{{ t('cfg.transferModeBlocks') }}</option></select></label>
      <label v-if="config.transferMode === 'time'"><span>{{ t('cfg.duration') }}</span><input type="number" :value="config.duration" min="1" max="86400" :disabled="clientRunning" @input="set('duration', Number(($event.target as HTMLInputElement).value))" /></label>
      <label v-else><span>{{ config.transferMode === 'bytes' ? t('cfg.transferAmountBytes') : t('cfg.transferAmountBlocks') }}</span><input type="number" :value="config.transferAmount" min="1" :disabled="clientRunning" @input="set('transferAmount', Number(($event.target as HTMLInputElement).value))" /><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.transferAmountNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.transferAmountNote'))" /></label>
      <label><span>{{ t('cfg.parallel') }}</span><input type="number" :value="config.parallel" min="1" max="128" :disabled="clientRunning" @input="set('parallel', Number(($event.target as HTMLInputElement).value))" /><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.parallelNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.parallelNote'))" /></label>
      <label><span>{{ t('cfg.bandwidth') }}</span><select :value="config.bandwidth" :disabled="clientRunning" @change="set('bandwidth', Number(($event.target as HTMLSelectElement).value))"><option v-for="option in bandwidthOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.unlimited'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.unlimited'))" /></label>
      <label><span>{{ t('cfg.tcpLen') }}</span><select :value="tcpPacketSelect" :disabled="clientRunning" @change="onPacketChange('tcp', $event)"><option v-for="option in tcpPacketOptions" :key="option.value" :value="option.value">{{ option.label }}</option><option value="custom">{{ t('cfg.custom') }}</option></select></label>
      <label><span>{{ t('cfg.udpLen') }}</span><select :value="udpPacketSelect" :disabled="clientRunning" @change="onPacketChange('udp', $event)"><option v-for="option in udpPacketOptions" :key="option.value" :value="option.value">{{ option.label }}</option><option value="custom">{{ t('cfg.custom') }}</option></select></label>
      <label class="log-option"><input type="checkbox" :checked="config.udpDontFragment" :disabled="clientRunning" @change="set('udpDontFragment', ($event.target as HTMLInputElement).checked)" /><span>{{ t('cfg.udpDontFragment') }}</span></label>
      <label class="log-option"><input type="checkbox" :checked="config.mptcp" :disabled="clientRunning" @change="set('mptcp', ($event.target as HTMLInputElement).checked)" /><span>{{ t('cfg.mptcp') }}</span></label>
      <label><span>{{ t('cfg.interval') }}</span><input type="number" :value="config.interval" min="1" max="60" :disabled="clientRunning" @input="set('interval', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>{{ t('cfg.omit') }}</span><input type="number" :value="config.omitSecs" min="0" :disabled="clientRunning" @input="set('omitSecs', Number(($event.target as HTMLInputElement).value))" /><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.omitNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.omitNote'))" /></label>
      <label><span>{{ t('cfg.window') }}</span><input type="number" :value="config.windowKb" min="0" max="16384" :disabled="clientRunning" @input="set('windowKb', Number(($event.target as HTMLInputElement).value))" /><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.windowNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.windowNote'))" /></label>
      <label><span>{{ t('cfg.cport') }}</span><input type="number" :value="config.cport" min="0" max="65535" :disabled="clientRunning" @input="set('cport', Number(($event.target as HTMLInputElement).value))" /><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.cportNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.cportNote'))" /></label>
      <label><span>{{ t('cfg.ipVersion') }}</span><select :value="config.ipVersion" :disabled="clientRunning" @change="set('ipVersion', Number(($event.target as HTMLSelectElement).value))"><option :value="0">{{ t('cfg.ipVersionAuto') }}</option><option :value="4">{{ t('cfg.ipVersion4') }}</option><option :value="6">{{ t('cfg.ipVersion6') }}</option></select></label>
      <label><span>{{ t('cfg.dscp') }}</span><select :value="config.dscp" :disabled="clientRunning" @change="set('dscp', Number(($event.target as HTMLSelectElement).value))"><option :value="0">{{ t('cfg.dscpDefault') }}</option><option v-for="option in dscpOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.dscpNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.dscpNote'))" /></label>
      <label><span>{{ t('cfg.congestion') }}</span><input :value="config.congestionAlgo" :disabled="clientRunning" :placeholder="t('cfg.congestionPlaceholder')" @input="set('congestionAlgo', ($event.target as HTMLInputElement).value)" /><Icon name="info" class="info-icon" :size="14" @mouseenter="showTip($event, t('cfg.congestionNote'))" @mousemove="moveTip" @mouseleave="hideTip" @click="pinTip($event, t('cfg.congestionNote'))" /></label>
      <label class="log-option"><input type="checkbox" :checked="config.getServerOutput" :disabled="clientRunning" @change="set('getServerOutput', ($event.target as HTMLInputElement).checked)" /><span>{{ t('cfg.getServerOutput') }}</span></label>
      <label><span>{{ t('cfg.engine') }}</span><select disabled><option>{{ t('cfg.engineValue') }}</option></select></label>
      <label><span>{{ t('cfg.direction') }}</span><select disabled><option>{{ t('cfg.directionValue') }}</option></select></label>
      <label class="log-option"><input type="checkbox" checked disabled /><span>{{ t('cfg.logOption') }}</span></label>
      <p class="runtime-state available">{{ t('cfg.engineReady') }}</p>
    </section>
    <section class="config-section auth-section">
      <div class="section-title"><h2>{{ t('cfg.auth') }}</h2></div>
      <label class="log-option"><input type="checkbox" :checked="config.authEnabled" :disabled="clientRunning" @change="set('authEnabled', ($event.target as HTMLInputElement).checked)" /><span>{{ t('cfg.authEnable') }}</span></label>
      <template v-if="config.authEnabled">
        <label><span>{{ t('cfg.authUser') }}</span><input :value="config.authUsername" :disabled="clientRunning" autocomplete="off" @input="set('authUsername', ($event.target as HTMLInputElement).value)" /></label>
        <label><span>{{ t('cfg.authPassword') }}</span><span class="field"><input type="password" :value="config.authPassword" :disabled="clientRunning" autocomplete="off" @input="set('authPassword', ($event.target as HTMLInputElement).value)" /><small>{{ t('cfg.authPasswordNote') }}</small></span></label>
        <label><span>{{ t('cfg.authKey') }}</span><span class="ip-row"><input :value="config.authPublicKeyPath" :disabled="clientRunning" :placeholder="t('cfg.authKeyPlaceholder')" @input="set('authPublicKeyPath', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="clientRunning" @click="emit('pick-public-key')">{{ t('cfg.authKeyBrowse') }}</button></span></label>
        <label class="log-option"><input type="checkbox" :checked="config.authPkcs1Padding" :disabled="clientRunning" @change="set('authPkcs1Padding', ($event.target as HTMLInputElement).checked)" /><span>{{ t('cfg.authPkcs1') }}</span></label>
        <p class="server-hint">{{ t('cfg.authHint') }}</p>
      </template>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="clientRunning" @click="emit('start')"><Icon name="play" />{{ t('cfg.start') }}</button>
      <button class="danger" :disabled="!clientRunning" @click="emit('stop')"><Icon name="stop" />{{ t('cfg.stop') }}</button>
    </div>
    </template>
    <template v-else>
    <section class="config-section server-params">
      <div class="section-title"><h2>{{ t('srv.title') }}</h2><span :class="['status-pill', serverRunning ? 'ok' : 'idle']">{{ serverRunning ? t('common.running') : t('common.notRunning') }}</span></div>
      <label><span>{{ t('srv.bindIp') }}</span><span class="ip-row"><input :value="serverConfig.bindIp" :disabled="serverRunning" :placeholder="t('srv.bindPlaceholder')" @input="setServer('bindIp', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="serverRunning" :title="t('nic.title')" @click="emit('pick-nic-server')">{{ t('cfg.nicBtn') }}</button></span></label>
      <label><span>{{ t('srv.port') }}</span><input type="number" :value="serverConfig.port" min="1" max="65535" :disabled="serverRunning" @input="setServer('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>{{ t('srv.interval') }}</span><span class="field"><input type="number" :value="serverConfig.interval" min="1" max="60" :disabled="serverRunning" @input="setServer('interval', Number(($event.target as HTMLInputElement).value))" /><small>{{ t('srv.intervalNote') }}</small></span></label>
      <label><span>{{ t('srv.idleTimeout') }}</span><span class="field"><input type="number" :value="serverConfig.idleTimeout" min="0" max="86400" :disabled="serverRunning" @input="setServer('idleTimeout', Number(($event.target as HTMLInputElement).value))" /><small>{{ t('srv.idleTimeoutNote') }}</small></span></label>
      <label><span>{{ t('srv.maxDuration') }}</span><span class="field"><input type="number" :value="serverConfig.maxDuration" min="0" max="86400" :disabled="serverRunning" @input="setServer('maxDuration', Number(($event.target as HTMLInputElement).value))" /><small>{{ t('srv.maxDurationNote') }}</small></span></label>
      <label><span>{{ t('srv.bitrateLimit') }}</span><span class="field"><input type="number" :value="serverConfig.bitrateLimit" min="0" max="1000000" :disabled="serverRunning" @input="setServer('bitrateLimit', Number(($event.target as HTMLInputElement).value))" /><small>{{ t('srv.bitrateLimitNote') }}</small></span></label>
      <p class="server-hint">{{ t('srv.hint') }}</p>
      <p class="runtime-state available">{{ t('cfg.engineReady') }}</p>
    </section>
    <section class="config-section auth-section">
      <div class="section-title"><h2>{{ t('srv.auth') }}</h2></div>
      <label class="log-option"><input type="checkbox" :checked="serverConfig.authEnabled" :disabled="serverRunning" @change="setServer('authEnabled', ($event.target as HTMLInputElement).checked)" /><span>{{ t('srv.authEnable') }}</span></label>
      <template v-if="serverConfig.authEnabled">
        <label><span>{{ t('srv.authKey') }}</span><span class="ip-row"><input :value="serverConfig.authPrivateKeyPath" :disabled="serverRunning" :placeholder="t('srv.authKeyPlaceholder')" @input="setServer('authPrivateKeyPath', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="serverRunning" :title="t('cfg.authKeyBrowse')" @click="emit('pick-auth-key')">{{ t('cfg.authKeyBrowse') }}</button></span></label>
        <label><span>{{ t('srv.authUsers') }}</span><span class="ip-row"><input :value="serverConfig.authUsersPath" :disabled="serverRunning" :placeholder="t('srv.authUsersPlaceholder')" @input="setServer('authUsersPath', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="serverRunning" :title="t('cfg.authKeyBrowse')" @click="emit('pick-auth-users')">{{ t('cfg.authKeyBrowse') }}</button></span></label>
        <label class="log-option"><input type="checkbox" :checked="serverConfig.authPkcs1Padding" :disabled="serverRunning" @change="setServer('authPkcs1Padding', ($event.target as HTMLInputElement).checked)" /><span>{{ t('cfg.authPkcs1') }}</span></label>
        <p class="server-hint">{{ t('srv.authHint') }}</p>
      </template>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="serverRunning" @click="emit('start-server')"><Icon name="play" />{{ t('srv.start') }}</button>
      <button class="danger" :disabled="!serverRunning" @click="emit('stop-server')"><Icon name="stop" />{{ t('srv.stop') }}</button>
    </div>
    <section class="config-section ssh-section">
      <div class="section-title"><h2>{{ t('ssh.title') }}</h2><span :class="['status-pill', sshStatus === 'connected' ? 'ok' : 'idle']">{{ sshStatus === 'connected' ? t('ssh.connected') : sshStatus === 'connecting' ? t('ssh.connecting') : t('ssh.notConnected') }}</span></div>
      <label><span>{{ t('ssh.host') }}</span><input :value="sshConfig.host" :disabled="sshLocked" :placeholder="t('ssh.hostPlaceholder')" @input="setSsh('host', ($event.target as HTMLInputElement).value)" /></label>
      <label><span>{{ t('ssh.port') }}</span><input type="number" :value="sshConfig.port" min="1" max="65535" :disabled="sshLocked" @input="setSsh('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>{{ t('ssh.user') }}</span><input :value="sshConfig.username" :disabled="sshLocked" autocomplete="off" @input="setSsh('username', ($event.target as HTMLInputElement).value)" /></label>
      <label><span>{{ t('ssh.auth') }}</span><select :value="sshConfig.authMethod" :disabled="sshLocked" @change="setSsh('authMethod', ($event.target as HTMLSelectElement).value as SshConfig['authMethod'])"><option value="password">{{ t('ssh.authPassword') }}</option><option value="key">{{ t('ssh.authKey') }}</option></select></label>
      <template v-if="sshConfig.authMethod === 'key'">
        <label><span>{{ t('ssh.key') }}</span><span class="ip-row"><input :value="sshConfig.privateKeyPath" :disabled="sshLocked" :placeholder="t('ssh.keyPlaceholder')" @input="setSsh('privateKeyPath', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="sshLocked" @click="emit('pick-private-key')">{{ t('cfg.authKeyBrowse') }}</button></span></label>
        <label><span>{{ t('ssh.passphrase') }}</span><span class="field"><input type="password" :value="sshConfig.passphrase" :disabled="sshLocked" autocomplete="off" @input="setSsh('passphrase', ($event.target as HTMLInputElement).value)" /><small>{{ t('ssh.secretNote') }}</small></span></label>
      </template>
      <label v-else><span>{{ t('ssh.password') }}</span><span class="field"><input type="password" :value="sshConfig.password" :disabled="sshLocked" autocomplete="off" @input="setSsh('password', ($event.target as HTMLInputElement).value)" /><small>{{ t('ssh.secretNote') }}</small></span></label>
      <p class="server-hint">{{ t('ssh.hint') }}</p>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="sshLocked" @click="emit('ssh-connect')"><Icon name="link" />{{ sshStatus === 'connecting' ? t('ssh.connecting') : t('ssh.connect') }}</button>
      <button class="danger" :disabled="sshStatus === 'idle'" @click="emit('ssh-disconnect')"><Icon name="unlink" />{{ t('ssh.disconnect') }}</button>
    </div>
    </template>
    <div v-if="customDialog" class="modal-backdrop" @click.self="customDialog = false">
      <div class="modal">
        <button class="modal-close" @click="customDialog = false">×</button>
        <h2>{{ t('cust.title', { protocol: customTarget === 'tcp' ? t('common.tcp') : t('common.udp') }) }}</h2>
        <div class="modal-body">
          <span class="modal-symbol info">i</span>
          <p class="custom-length-form">
            <input type="number" v-model.number="customValue" min="1" :placeholder="t('cust.value')" />
            <select v-model="customUnit"><option value="bytes">Bytes</option><option value="kb">KB</option><option value="mb">MB</option></select>
          </p>
          <p class="custom-length-result">{{ t('cust.result', { bytes: customBytes, max: customTarget === 'tcp' ? '1048576 (1MB)' : '65536 (64KB)' }) }}</p>
        </div>
        <div class="modal-actions">
          <button @click="customDialog = false">{{ t('common.cancel') }}</button>
          <button class="primary" @click="confirmCustomLength">{{ t('common.confirm') }}</button>
        </div>
      </div>
    </div>
    <div v-if="tooltip" class="item-tooltip" :style="{ left: tooltip.x + 'px', top: tooltip.y + 'px' }">{{ tooltip.text }}</div>
  </aside>
</template>
