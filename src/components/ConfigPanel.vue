<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import type { IperfRuntimeInfo, NetworkInfo, TestConfig, TestItem } from '../types'
import Icon from './Icon.vue'

const props = defineProps<{ config: TestConfig; items: TestItem[]; running: boolean; recovery?: boolean; runtime?: IperfRuntimeInfo; local: NetworkInfo; savedCustomLength: number }>()
const emit = defineEmits<{
  'update:config': [value: TestConfig]
  'toggle-item': [id: string]
  reset: []
  start: []
  stop: []
  report: []
  clear: []
  'pick-nic': []
  'save-custom-length': [value: number]
}>()

const set = <K extends keyof TestConfig>(key: K, value: TestConfig[K]) => emit('update:config', { ...props.config, [key]: value })

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
    <section class="config-section tests-section">
      <h2>测试项目</h2>
      <div class="test-list">
        <label v-for="(item, index) in items" :key="item.id" :class="{ muted: item.protocol !== 'ping' && item.protocol !== config.protocol }">
          <input type="checkbox" :checked="item.enabled" :disabled="running || (item.protocol !== 'ping' && item.protocol !== config.protocol)" @change="emit('toggle-item', item.id)" />
          <span>{{ index + 1 }}. {{ item.label }}</span><span class="drag">≡</span>
        </label>
      </div>
    </section>
    <section class="config-section parameters">
      <div class="section-title"><h2>参数设置</h2><button class="text-button" :disabled="running" @click="emit('reset')">↻ 重置</button></div>
      <label><span>服务端 IP</span><span class="ip-row"><input :value="config.serverIp" :disabled="running || config.mode === 'server'" @input="set('serverIp', ($event.target as HTMLInputElement).value)" /><button class="mini-button" type="button" :disabled="running" title="选择本机网卡" @click="emit('pick-nic')">本机</button></span></label>
      <label><span>端口</span><input type="number" :value="config.port" min="1" max="65535" :disabled="running" @input="set('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>持续时间(s)</span><input type="number" :value="config.duration" min="1" max="86400" :disabled="running" @input="set('duration', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>并发流数</span><input type="number" :value="config.parallel" min="1" max="128" :disabled="running || config.protocol === 'udp'" @input="set('parallel', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>带宽限制</span><select :value="config.bandwidth" :disabled="running" @change="set('bandwidth', Number(($event.target as HTMLSelectElement).value))"><option v-for="option in bandwidthOptions" :key="option.value" :value="option.value">{{ option.label }}</option></select><small>0 = 不限制</small></label>
      <label><span>报文长度</span><select :value="packetSelectValue" :disabled="running" @change="onPacketChange"><option v-for="option in packetOptions" :key="option.value" :value="option.value">{{ option.label }}</option><option value="custom">自定义…</option></select></label>
      <label v-if="customMode"><span>自定义长度</span><span class="ip-row"><input type="number" v-model.number="customInput" min="1" max="262144" :disabled="running" placeholder="bytes" /><button class="mini-button" type="button" :disabled="running" @click="confirmCustomLength">保存</button></span><small>1 ~ 262144 bytes</small></label>
      <label><span>间隔输出(s)</span><input type="number" :value="config.interval" min="1" max="60" :disabled="running" @input="set('interval', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>协议版本</span><select disabled><option>{{ runtime?.available ? runtime.version : 'iperf3 不可用' }}</option></select></label>
      <label><span>传输方向</span><select disabled><option>正向（默认）</option></select></label>
      <label class="log-option"><input type="checkbox" checked disabled /><span>测试中出现非致命错误仅记录日志</span></label>
      <p :class="['runtime-state', runtime?.available ? 'available' : 'missing']">{{ runtime?.available ? `● ${runtime.bundled ? '内置运行时已就绪' : '系统运行时已就绪'}` : '● 未找到 iperf3 运行时' }}</p>
    </section>
    <div class="config-actions">
      <button class="primary" :disabled="running" @click="emit('start')"><Icon name="play" />{{ recovery ? '恢复测试' : (config.mode === 'server' ? '启动服务' : '开始测试') }}</button>
      <button class="danger" :disabled="!running" @click="emit('stop')"><Icon name="stop" />停止测试</button>
      <button class="success" :disabled="running" @click="emit('report')"><Icon name="report" />生成报告</button>
      <button :disabled="running" @click="emit('clear')"><Icon name="trash" />清空日志</button>
    </div>
  </aside>
</template>
