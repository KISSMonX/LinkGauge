/**
 * Tooltip composable: fixed-position floating tooltip that follows the mouse
 * on hover (test item descriptions, parameter annotations), with click-to-pin.
 *
 * Extracted from ConfigPanel.vue (~22 lines). Self-contained — no external
 * dependencies beyond Vue reactivity.
 */
import { ref } from 'vue'

export function useTooltip() {
  const tooltip = ref<{ text: string; x: number; y: number; pinned: boolean } | null>(null)

  /** Clamp tooltip position so it stays within the viewport. */
  const clampTip = (x: number, y: number) => ({
    x: Math.min(x + 14, window.innerWidth - 280),
    y: Math.min(y + 14, window.innerHeight - 70),
  })

  function showTip(event: MouseEvent, text: string) {
    tooltip.value = { text, ...clampTip(event.clientX, event.clientY), pinned: false }
  }

  function moveTip(event: MouseEvent) {
    if (tooltip.value && !tooltip.value.pinned) {
      tooltip.value = { ...tooltip.value, ...clampTip(event.clientX, event.clientY) }
    }
  }

  /** Click toggles pin: second click on the same tip closes it; pinned tips don't hide on mouse-leave. */
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

  return { tooltip, showTip, moveTip, pinTip, hideTip }
}
