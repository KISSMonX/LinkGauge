/**
 * Tab drag-to-detach composable: pointer-event-based tab button dragging.
 * When dragged beyond a threshold, the tab detaches into a separate window.
 *
 * Extracted from ConfigPanel.vue (~55 lines). Manages ghost element creation,
 * pointer capture, blur cleanup, and suppressed-click after detach.
 */
import { onMounted, onUnmounted, ref } from 'vue'

const DETACH_THRESHOLD = 100

export function useTabDrag(
  t: (...args: any[]) => string,
  tabs: ('client' | 'server')[] | undefined,
  onDetach: (side: 'client' | 'server') => void,
) {
  const isTauri = () => '__TAURI_INTERNALS__' in window
  const drag = ref<{ side: 'client' | 'server'; startX: number; startY: number; ghost: HTMLElement } | null>(null)
  const dragFar = ref(false)
  const suppressedClick = ref(false)

  function startTabDrag(side: 'client' | 'server', event: PointerEvent) {
    if (!isTauri() || !tabs?.length || event.button !== 0) return
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
    if (event.type !== 'pointerup') return
    if (Math.hypot(event.clientX - d.startX, event.clientY - d.startY) > DETACH_THRESHOLD) {
      suppressedClick.value = true
      onDetach(d.side)
    }
  }

  function onTabClick() {
    if (suppressedClick.value) { suppressedClick.value = false; return }
  }

  function cancelTabDrag() {
    const d = drag.value
    if (!d) return
    drag.value = null
    dragFar.value = false
    d.ghost.remove()
  }

  onMounted(() => { window.addEventListener('blur', cancelTabDrag) })
  onUnmounted(() => { window.removeEventListener('blur', cancelTabDrag) })

  return { dragFar, suppressedClick, startTabDrag, onTabDragMove, endTabDrag, onTabClick }
}
