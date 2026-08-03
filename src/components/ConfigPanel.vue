<script setup lang="ts">
import type { IperfRuntimeInfo, TestConfig, TestItem } from '../types'
import Icon from './Icon.vue'

const props = defineProps<{ config: TestConfig; items: TestItem[]; running: boolean; recovery?: boolean; runtime?: IperfRuntimeInfo }>()
const emit = defineEmits<{
  'update:config': [value: TestConfig]
  'toggle-item': [id: string]
  reset: []
  start: []
  stop: []
  report: []
  clear: []
}>()

const set = <K extends keyof TestConfig>(key: K, value: TestConfig[K]) => emit('update:config', { ...props.config, [key]: value })
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
      <label><span>服务端 IP</span><input :value="config.serverIp" :disabled="running || config.mode === 'server'" @input="set('serverIp', ($event.target as HTMLInputElement).value)" /></label>
      <label><span>端口</span><input type="number" :value="config.port" min="1" max="65535" :disabled="running" @input="set('port', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>持续时间(s)</span><input type="number" :value="config.duration" min="1" max="86400" :disabled="running" @input="set('duration', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>并发流数</span><input type="number" :value="config.parallel" min="1" max="128" :disabled="running || config.protocol === 'udp'" @input="set('parallel', Number(($event.target as HTMLInputElement).value))" /></label>
      <label><span>带宽限制(Mbps)</span><input type="number" :value="config.bandwidth" min="0" :disabled="running || config.protocol !== 'udp'" @input="set('bandwidth', Number(($event.target as HTMLInputElement).value))" /><small>0 = 不限制</small></label>
      <label><span>报文长度</span><select :value="config.packetLength" :disabled="running" @change="set('packetLength', Number(($event.target as HTMLSelectElement).value))"><option :value="128">默认 (128 KB)</option><option :value="512">512 bytes</option><option :value="1200">1200 bytes</option><option :value="1472">1472 bytes</option></select></label>
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
