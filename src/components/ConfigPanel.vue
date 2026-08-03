<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import type { NetworkInfo, ServerConfig, TestConfig, TestItem } from '../types'
import Icon from './Icon.vue'

/** tabs：主窗口已停靠的标签列表（undefined = 分离窗口，仅显示 detached 一侧） */
const props = defineProps<{ tabs?: ('client' | 'server')[]; detached?: 'client' | 'server'; tab: 'client' | 'server'; config: TestConfig; serverConfig: ServerConfig; items: TestItem[]; clientRunning: boolean; serverRunning: boolean; recovery?: boolean; local: NetworkInfo; savedCustomLength: number; savedCustomUdpLength: number }>()
const emit = defineEmits<{
  'update:tab': [value: 'client' | 'server']
  'update:config': [value: TestConfig]
  'update:server-config': [value: ServerConfig]
  'toggle-item': [id: string]
  reset: []
  start: []
  stop: []
  'start-server': []
  'stop-server': []
  clear: []
  'pick-nic': []
  'pick-nic-server': []
  'save-custom-length': [protocol: 'tcp' | 'udp', value: number]
  /** 标签页被拖拽分离为独立窗口 */
  detach: [side: 'client' | 'server']
}>()

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
  ghost.textContent = side === 'client' ? '客户端' : '服务端'
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
  d.ghost.textContent = d.side === 'client' ? '客户端' : '服务端'
  d.ghost.classList.toggle('detach', dragFar.value)
  if (dragFar.value) d.ghost.textContent += ' ⇱ 释放以分离'
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

/** 带宽限制选项：当前网卡速率（默认） + 100 / 1000 / 0（不限制），相同速率去重 */
const bandwidthOptions = computed(() => {
  const nic = props.local.speedMbps > 0 ? props.local.speedMbps : 0
  const options: { value: number; label: string }[] = []
  for (const value of [nic, 100, 1000, 0]) {
    if (options.some((option) => option.value === value)) continue
    const label = value === nic && nic > 0 ? `${value} Mbps（当前网卡）` : value === 0 ? '0 Mbps（不限制）' : `${value} Mbps`
    options.push({ value, label })
  }
  return options
})

// TCP 报文长度预设（最大 1MB，默认 128KB）
const TCP_PRESETS = [1024, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576]
// UDP 报文长度预设（最大 64KB，默认 8KB）
const UDP_PRESETS = [128, 512, 1024, 1472, 4096, 8192, 16384, 32768, 65536]
const formatLength = (value: number) => (value >= 1024 && value % 1024 === 0 ? `${value / 1024} KB` : `${value} bytes`)
const isPreset = (presets: number[], value: number) => presets.includes(value)

/** 报文长度下拉选项：预设值 + 已保存的自定义值 + 「自定义…」 */
function lengthOptions(presets: number[], savedCustom: number) {
  const options = presets.map((value) => ({ value, label: formatLength(value) }))
  if (savedCustom > 0 && !isPreset(presets, savedCustom)) {
    options.push({ value: savedCustom, label: `${formatLength(savedCustom)}（自定义）` })
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
    : (props.config.udpPacketLength > 0 && !isPreset(UDP_PRESETS, props.config.udpPacketLength) ? props.config.udpPacketLength : (props.savedCustomUdpLength || 8192))
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
      <button v-for="side in tabs" :key="side" :class="{ active: tab === side }" title="拖拽标签可分离为独立窗口" @pointerdown="startTabDrag(side, $event)" @pointermove="onTabDragMove" @pointerup="endTabDrag" @pointercancel="endTabDrag" @click="onTabClick(); emit('update:tab', side)"><Icon name="monitor" />{{ side === 'client' ? '客户端' : '服务端' }}<span class="tab-detach">⇱</span></button>
    </div>
    <template v-if="visibleSide === 'client'">
    <section class="config-section tests-section">
      <div class="section-title"><h2>测试项目</h2></div>
      <p class="protocol-hint">TCP 与 UDP 测试项可同时勾选，将按列表顺序逐个执行</p>
      <div class="test-list">
        <label v-for="(item, index) in items" :key="item.id">
          <input type="checkbox" :checked="item.enabled" :disabled="clientRunning" @change="emit('toggle-item', item.id)" />
          <span>{{ index + 1 }}. {{ item.label }}</span><span class="drag">≡</span>
        </label>
      </div>
    </section>
    <section class="config-section parameters">
      <div class="section-title"><h2>参数设置</h2><button class="text-button" :disabled="clientRunning" @click="emit('reset')">↻ 重置</button></div>
      <label><span>服务端 IP</span><span class="ip-row"><input :value="config.serverIp" :disabled="clientRunning" @input="set('serverIp', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="clientRunning" title="选择本机网卡" @click="emit('pick-nic')">本机</button></span></label>
      <label><span>端口</span><input type="number" :value="config.port" min="1" max="65535" :disabled="clientRunning" @input="set('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>持续时间(s)</span><input type="number" :value="config.duration" min="1" max="86400" :disabled="clientRunning" @input="set('duration', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>并发流数</span><input type="number" :value="config.parallel" min="1" max="128" :disabled="clientRunning" @input="set('parallel', Number(($event.target as HTMLInputElement).value))" /><small>仅 TCP 多并发流生效</small></label>
      <label><span>带宽限制</span><select :value="config.bandwidth" :disabled="clientRunning" @change="set('bandwidth', Number(($event.target as HTMLSelectElement).value))"><option v-for="option in bandwidthOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select><small>0 = 不限制</small></label>
      <label><span>TCP 报文长度</span><select :value="tcpPacketSelect" :disabled="clientRunning" @change="onPacketChange('tcp', $event)"><option v-for="option in tcpPacketOptions" :key="option.value" :value="option.value">{{ option.label }}</option><option value="custom">自定义…</option></select></label>
      <label><span>UDP 报文长度</span><select :value="udpPacketSelect" :disabled="clientRunning" @change="onPacketChange('udp', $event)"><option v-for="option in udpPacketOptions" :key="option.value" :value="option.value">{{ option.label }}</option><option value="custom">自定义…</option></select></label>
      <label><span>间隔输出(s)</span><input type="number" :value="config.interval" min="1" max="60" :disabled="clientRunning" @input="set('interval', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>测试引擎</span><select disabled><option>riperf3（纯 Rust 内置）</option></select></label>
      <label><span>传输方向</span><select disabled><option>正向（默认）</option></select></label>
      <label class="log-option"><input type="checkbox" checked disabled /><span>测试中出现非致命错误仅记录日志</span></label>
      <p class="runtime-state available">● 内置 riperf3 引擎已就绪（无需安装 iperf3）</p>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="clientRunning" @click="emit('start')"><Icon name="play" />{{ recovery ? '恢复测试' : '开始测试' }}</button>
      <button class="danger" :disabled="!clientRunning" @click="emit('stop')"><Icon name="stop" />停止测试</button>
    </div>
    </template>
    <template v-else>
    <section class="config-section server-params">      <div class="section-title"><h2>服务端设置</h2><span :class="['status-pill', serverRunning ? 'ok' : 'idle']">{{ serverRunning ? '运行中' : '未运行' }}</span></div>
      <label><span>绑定 IP</span><span class="ip-row"><input :value="serverConfig.bindIp" :disabled="serverRunning" placeholder="留空 = 所有网卡" @input="setServer('bindIp', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="serverRunning" title="选择本机网卡" @click="emit('pick-nic-server')">本机</button></span></label>
      <label><span>监听端口</span><input type="number" :value="serverConfig.port" min="1" max="65535" :disabled="serverRunning" @input="setServer('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>日志输出间隔(s)</span><input type="number" :value="serverConfig.interval" min="1" max="60" :disabled="serverRunning" @input="setServer('interval', Number(($event.target as HTMLInputElement).value))" /><small>每隔几秒输出一次日志与统计</small></label>
      <p class="server-hint">服务端将持续监听配置的端口并处理测试请求，客户端与服务端可同时运行（同机测试时客户端连本机 IP 与同一端口）。绑定 IP 留空时监听所有网卡。</p>
      <p class="runtime-state available">● 内置 riperf3 引擎已就绪（无需安装 iperf3）</p>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="serverRunning" @click="emit('start-server')"><Icon name="play" />启动服务</button>
      <button class="danger" :disabled="!serverRunning" @click="emit('stop-server')"><Icon name="stop" />停止服务</button>
    </div>
    </template>
    <div v-if="customDialog" class="modal-backdrop" @click.self="customDialog = false">
      <div class="modal">
        <button class="modal-close" @click="customDialog = false">×</button>
        <h2>自定义{{ customTarget === 'tcp' ? 'TCP' : 'UDP' }}报文长度</h2>
        <div class="modal-body">
          <span class="modal-symbol info">i</span>
          <p class="custom-length-form">
            <input type="number" v-model.number="customValue" min="1" placeholder="数值" />
            <select v-model="customUnit"><option value="bytes">Bytes</option><option value="kb">KB</option><option value="mb">MB</option></select>
          </p>
          <p class="custom-length-result">换算：<b>{{ customBytes }}</b> bytes（范围 1 ~ {{ customTarget === 'tcp' ? '1048576 (1MB)' : '65536 (64KB)' }}）</p>
        </div>
        <div class="modal-actions">
          <button @click="customDialog = false">取消</button>
          <button class="primary" @click="confirmCustomLength">确定</button>
        </div>
      </div>
    </div>
  </aside>
</template>
