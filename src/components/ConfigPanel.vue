<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { NetworkInfo, ServerConfig, TestConfig, TestItem } from '../types'
import Icon from './Icon.vue'

const props = defineProps<{ tab: 'client' | 'server'; config: TestConfig; serverConfig: ServerConfig; items: TestItem[]; clientRunning: boolean; serverRunning: boolean; recovery?: boolean; local: NetworkInfo; savedCustomLength: number }>()
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
  'save-custom-length': [value: number]
}>()

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

const PACKET_PRESETS = [128, 512, 1024, 1200, 1472, 4096, 8192, 16384, 32768, 65536]
const formatLength = (value: number) => (value >= 1024 && value % 1024 === 0 ? `${value / 1024} KB` : `${value} bytes`)
const isPreset = (value: number) => PACKET_PRESETS.includes(value)

/** 报文长度下拉：预设值 + 已保存的自定义值 + 「自定义…」 */
const packetOptions = computed(() => {
  const options = PACKET_PRESETS.map((value) => ({ value, label: formatLength(value) }))
  if (props.savedCustomLength > 0 && !isPreset(props.savedCustomLength)) {
    options.push({ value: props.savedCustomLength, label: `${formatLength(props.savedCustomLength)}（自定义）` })
  }
  return options
})

/** select 取值：能匹配选项就用数字，否则为 'custom' */
const packetSelectValue = computed(() => (packetOptions.value.some((option) => option.value === props.config.packetLength) ? props.config.packetLength : 'custom'))
const customMode = computed(() => packetSelectValue.value === 'custom')
const customInput = ref<number | null>(null)
watch(packetSelectValue, (value) => {
  if (value === 'custom' && customInput.value === null) {
    customInput.value = props.config.packetLength > 0 && !isPreset(props.config.packetLength) ? props.config.packetLength : (props.savedCustomLength || 1024)
  }
}, { immediate: true })

function onPacketChange(event: Event) {
  const value = (event.target as HTMLSelectElement).value
  if (value === 'custom') {
    customInput.value = props.config.packetLength > 0 && !isPreset(props.config.packetLength) ? props.config.packetLength : (props.savedCustomLength || 1024)
  } else {
    set('packetLength', Number(value))
  }
}
function confirmCustomLength() {
  const length = customInput.value
  if (length === null || !Number.isFinite(length) || length < 1 || length > 262144) { customInput.value = null; return }
  customInput.value = null
  emit('save-custom-length', Math.round(length))
}
</script>

<template>
  <aside class="panel config-panel">
    <div class="mode-tabs">
      <button :class="{ active: tab === 'client' }" @click="emit('update:tab', 'client')"><Icon name="monitor" />客户端</button>
      <button :class="{ active: tab === 'server' }" @click="emit('update:tab', 'server')"><Icon name="monitor" />服务端</button>
    </div>
    <template v-if="tab === 'client'">
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
      <label><span>报文长度</span><select :value="packetSelectValue" :disabled="clientRunning" @change="onPacketChange"><option v-for="option in packetOptions" :key="option.value" :value="option.value">{{ option.label }}</option><option value="custom">自定义…</option></select></label>
      <label v-if="customMode"><span>自定义长度</span><span class="ip-row"><input type="number" v-model.number="customInput" min="1" max="262144" :disabled="clientRunning" placeholder="bytes" /><button class="mini-button" type="button" :disabled="clientRunning" @click="confirmCustomLength">保存</button></span><small>1 ~ 262144 bytes</small></label>
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
    <section class="config-section server-params">
      <div class="section-title"><h2>服务端设置</h2><span :class="['status-pill', serverRunning ? 'ok' : 'idle']">{{ serverRunning ? '运行中' : '未运行' }}</span></div>
      <label><span>监听端口</span><input type="number" :value="serverConfig.port" min="1" max="65535" :disabled="serverRunning" @input="setServer('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <p class="server-hint">服务端将持续监听配置的端口并处理测试请求，客户端与服务端可同时运行（同机测试时客户端连本机 IP 与同一端口）。</p>
      <p class="runtime-state available">● 内置 riperf3 引擎已就绪（无需安装 iperf3）</p>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="serverRunning" @click="emit('start-server')"><Icon name="play" />启动服务</button>
      <button class="danger" :disabled="!serverRunning" @click="emit('stop-server')"><Icon name="stop" />停止服务</button>
      <button @click="emit('clear')"><Icon name="trash" />清空日志</button>
    </div>
    </template>
  </aside>
</template>
