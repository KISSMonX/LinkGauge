/**
 * 大数值自适应单位格式化:概览指标(带宽/传输量)数值较大时自动升级单位,
 * 避免超出单元格宽度被省略号截断(如 "23453...")。
 * 精确值仍可通过 title 悬停查看。
 */

/** 带宽格式化(输入 Mbps):23453.67 → "23.45 Gbps",≥1e6 → Tbps */
export function formatBandwidth(mbps: number): string {
  if (mbps >= 1_000_000) return `${(mbps / 1_000_000).toFixed(2)} Tbps`
  if (mbps >= 1000) return `${(mbps / 1000).toFixed(2)} Gbps`
  return `${mbps.toFixed(2)} Mbps`
}

/** 传输量格式化(输入 MB):52340 → "52.34 GB",≥1e6 → TB */
export function formatTransfer(mb: number): string {
  if (mb >= 1_000_000) return `${(mb / 1_000_000).toFixed(2)} TB`
  if (mb >= 1000) return `${(mb / 1000).toFixed(2)} GB`
  return `${mb.toFixed(2)} MB`
}

/** 秒数 → H:MM:SS 格式（hours 段仅在 ≥1 小时时显示） */
export function formatTime(s: number): string {
  const h = Math.floor(s / 3600)
  const m = Math.floor((s % 3600) / 60)
  const sec = s % 60
  const mmss = `${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}`
  return h > 0 ? `${String(h).padStart(2, '0')}:${mmss}` : `00:${mmss}`
}
