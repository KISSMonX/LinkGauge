/** SSH 控制台的行缓冲。
 *
 *  后端已剔除 ANSI 转义序列，这里只需实现单行光标语义：
 *  \n 换行、\r 回到行首（后续字符覆盖本行）、\b 退格、\t 制表位对齐。
 *  这足以正确显示 shell 回显、iperf3 的逐秒统计与就地覆盖式进度输出，
 *  又不必引入完整的终端模拟器依赖。 */
export interface Terminal {
  lines: string[]
  /** 当前行内的光标列（UTF-16 code unit 计） */
  col: number
}

/** 保留的最大行数，超出后从最旧的行开始丢弃 */
const MAX_LINES = 2000
/** 制表位宽度 */
const TAB_WIDTH = 8

export function createTerminal(): Terminal {
  return { lines: [''], col: 0 }
}

export function clearTerminal(term: Terminal) {
  term.lines = ['']
  term.col = 0
}

export function writeTerminal(term: Terminal, text: string) {
  if (!text) return
  let line = term.lines[term.lines.length - 1] ?? ''
  for (const ch of text) {
    if (ch === '\n') {
      term.lines[term.lines.length - 1] = line
      term.lines.push('')
      line = ''
      term.col = 0
    } else if (ch === '\r') {
      term.col = 0
    } else if (ch === '\b') {
      term.col = Math.max(0, term.col - 1)
    } else if (ch === '\t') {
      const target = term.col + (TAB_WIDTH - (term.col % TAB_WIDTH))
      if (line.length < target) line = line.padEnd(target, ' ')
      term.col = target
    } else {
      // 光标在行尾之后时先补空格，否则覆盖光标处的字符
      line = line.length < term.col
        ? line.padEnd(term.col, ' ') + ch
        : line.slice(0, term.col) + ch + line.slice(term.col + ch.length)
      term.col += ch.length
    }
  }
  term.lines[term.lines.length - 1] = line
  if (term.lines.length > MAX_LINES) term.lines.splice(0, term.lines.length - MAX_LINES)
}
