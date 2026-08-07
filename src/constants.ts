/** TCP 报文长度预设（最大 1MB，默认 128KB） */
export const TCP_PRESETS = [1024, 4096, 8192, 16384, 32768, 65536, 131072, 262144, 524288, 1048576]

/** UDP 报文长度预设（最大 64KB，默认 1460 = iperf3 的 DEFAULT_UDP_BLKSIZE） */
export const UDP_PRESETS = [128, 512, 1024, 1460, 1472, 4096, 8192, 16384, 32768, 65536]

/** DSCP 常用取值（0-63，--dscp；标签为协议常量） */
export const dscpOptions = [
  { value: 8, label: 'CS1 (8)' }, { value: 16, label: 'CS2 (16)' }, { value: 24, label: 'CS3 (24)' },
  { value: 32, label: 'CS4 (32)' }, { value: 40, label: 'CS5 (40)' }, { value: 48, label: 'CS6 (48)' },
  { value: 56, label: 'CS7 (56)' }, { value: 46, label: 'EF (46)' }, { value: 44, label: 'VA (44)' },
  { value: 10, label: 'AF11 (10)' }, { value: 18, label: 'AF21 (18)' }, { value: 26, label: 'AF31 (26)' },
  { value: 34, label: 'AF41 (34)' }, { value: 1, label: 'LE (1)' },
]
