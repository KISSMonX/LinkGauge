<script setup lang="ts">
import type { AppMode, Protocol } from '../types'
import Icon from './Icon.vue'

defineProps<{ mode: AppMode; protocol: Protocol }>()
const emit = defineEmits<{
  'update:mode': [value: AppMode]
  'update:protocol': [value: Protocol]
  import: []
  export: []
  save: []
  settings: []
  about: []
}>()
</script>

<template>
  <header class="topbar">
    <button class="icon-button menu-button" title="菜单"><Icon name="menu" :size="21" /></button>
    <div class="segmented mode-switch">
      <button :class="{ active: mode === 'server' }" @click="emit('update:mode', 'server')"><Icon name="monitor" />服务端</button>
      <button :class="{ active: mode === 'client' }" @click="emit('update:mode', 'client')"><Icon name="monitor" />客户端</button>
    </div>
    <div class="segmented protocol-switch">
      <button :class="{ active: protocol === 'tcp' }" @click="emit('update:protocol', 'tcp')">TCP 测试</button>
      <button :class="{ active: protocol === 'udp' }" @click="emit('update:protocol', 'udp')">UDP 测试</button>
    </div>
    <nav class="toolbar-actions">
      <button @click="emit('import')"><Icon name="download" />导入配置</button>
      <button @click="emit('export')"><Icon name="upload" />导出配置</button>
      <button @click="emit('save')"><Icon name="save" />保存配置</button>
      <button @click="emit('settings')"><Icon name="settings" />设置</button>
      <button @click="emit('about')"><Icon name="info" />关于</button>
    </nav>
  </header>
</template>
