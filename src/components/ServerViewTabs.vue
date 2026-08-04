<script setup lang="ts">
/** 服务端视角中间栏的视图切换：服务端概览 ↔ SSH 远程控制台。
 *  两个面板各自在标题栏渲染同一组标签，因此抽成共用组件避免走样。 */
import type { SshStatus } from '../types'
import { useI18n } from '../i18n'
import Icon from './Icon.vue'

defineProps<{ view: 'overview' | 'ssh'; sshStatus: SshStatus }>()
const emit = defineEmits<{ 'update:view': [value: 'overview' | 'ssh'] }>()
const { t } = useI18n()
</script>

<template>
  <div class="view-tabs">
    <button :class="{ active: view === 'overview' }" @click="emit('update:view', 'overview')"><Icon name="monitor" :size="15" />{{ t('sdash.title') }}</button>
    <button :class="{ active: view === 'ssh' }" :title="t('ssh.consoleHint')" @click="emit('update:view', 'ssh')"><Icon name="terminal" :size="15" />{{ t('ssh.console') }}<span :class="['ssh-dot', sshStatus]" /></button>
  </div>
</template>
