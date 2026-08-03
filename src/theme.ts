import { ref } from 'vue'

export type Theme = 'light' | 'dark'

/** 主题外观：默认亮色，持久化到本地存储 */
const savedTheme = localStorage.getItem('linkgauge-theme')
export const theme = ref<Theme>(savedTheme === 'dark' ? 'dark' : 'light')

export function setTheme(value: Theme) {
  theme.value = value
  localStorage.setItem('linkgauge-theme', value)
  applyTheme()
}

function applyTheme() {
  document.documentElement.dataset.theme = theme.value
}

applyTheme()
