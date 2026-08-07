# Changelog

## [0.2.1](https://github.com/KISSMonX/LinkGauge/compare/v0.1.1...v0.2.1) (2026-08-07)


### Features

* add --get-server-output client option ([a43aa71](https://github.com/KISSMonX/LinkGauge/commit/a43aa71e65fef5f080b72fc20b979f43c73acfd7))
* add byte/block, MPTCP and DF test items to the queue ([c2a0add](https://github.com/KISSMonX/LinkGauge/commit/c2a0adddf339213afaa8d5966fd9ba03802345b3))
* add client source port (--cport) and IPv4/IPv6 selection options ([41a8f4c](https://github.com/KISSMonX/LinkGauge/commit/41a8f4cb28a69cba3cb3e1880832494eaeef9aaf))
* add congestion control (-C) and UDP don't-fragment options ([68ad7c3](https://github.com/KISSMonX/LinkGauge/commit/68ad7c39701f792c8c6ded001c054d2cccbaeb22))
* add DSCP marking and byte/block-limited tests (-n/-k) ([da62188](https://github.com/KISSMonX/LinkGauge/commit/da621888ea4b2c2b5055609eeda0e4c9ef4d8b62))
* add MPTCP multipath client option ([12a68cd](https://github.com/KISSMonX/LinkGauge/commit/12a68cd3feb275700236d2ef98c55c6b5fca5852))
* add server-side protection options (idle timeout, max duration, rate cap) ([ee7d866](https://github.com/KISSMonX/LinkGauge/commit/ee7d866a5466115aa04903d3157b5ca2173027d8))
* add warm-up omit (-O) and socket buffer (-w) client options ([c0e303f](https://github.com/KISSMonX/LinkGauge/commit/c0e303f4af270db4906df127c5170ac4ad6e68cf))
* complete iperf3 option set — auth, protection, advanced params, expanded test queue ([2cec677](https://github.com/KISSMonX/LinkGauge/commit/2cec677f5ac721dec02a2a0a8db8db6df3de720f))
* enlarge info icons, theme-contrast i mark ([96590db](https://github.com/KISSMonX/LinkGauge/commit/96590dbff6f2b13af7acb5c5151f7766eb77b968))
* filled info icons in both themes ([cf77c46](https://github.com/KISSMonX/LinkGauge/commit/cf77c464e8cbc6aeadbbcc2bcca74857e9700c8c))
* **log:** append frontend queue errors to client run logs ([7938f8d](https://github.com/KISSMonX/LinkGauge/commit/7938f8de83e6f62165775742211bb12a5a08d826))
* **log:** 客户端汇总文件记录完整执行过程 ([1914c27](https://github.com/KISSMonX/LinkGauge/commit/1914c27743f8d5a8a0beb83c9798350eb06df32e))
* **log:** 日志文件名随界面语言 + 客户端运行汇总文件 ([96f3acd](https://github.com/KISSMonX/LinkGauge/commit/96f3acd0bc7a5b71d86961ac6f9ae83f781d7015))
* make the server stats sampling interval configurable ([c7d541a](https://github.com/KISSMonX/LinkGauge/commit/c7d541a62b0c441f50888d403aea7cef4a6e1407))
* move auth/server/SSH notes into info icons too ([0dc00a0](https://github.com/KISSMonX/LinkGauge/commit/0dc00a0a6c34eb578fed8c6b361936e046633d85))
* move checkbox option details into info icons ([8281a77](https://github.com/KISSMonX/LinkGauge/commit/8281a7727f383419b32e3b6e2263d95ede1ab113))
* move parameter notes into hover/click info icons ([4166b46](https://github.com/KISSMonX/LinkGauge/commit/4166b46ccb2009dbf0f935a915adb0ef8e4aaa09))
* **queue:** 事件续期制看门狗 + 首事件探针升级 + 驱动死亡守卫 + 重复开始守卫 ([b7de894](https://github.com/KISSMonX/LinkGauge/commit/b7de894c649d55898d3ff0e6ed7e66022c5c77da))
* **queue:** 首事件探针——任务启动 5 秒无事件立即告警 ([8cbcf02](https://github.com/KISSMonX/LinkGauge/commit/8cbcf023e6dcf345cab97fb41e6517447e46e26e))
* **report:** print PDF from HTML ([d625476](https://github.com/KISSMonX/LinkGauge/commit/d6254761b7961ee91b44ed5c9b8e20e323049303))
* show test-item descriptions on hover ([1943869](https://github.com/KISSMonX/LinkGauge/commit/194386954f7a0a66f764fe0971d064eb9c388fb4))
* show transfer amount input when byte/block test items are checked ([2cfbf8c](https://github.com/KISSMonX/LinkGauge/commit/2cfbf8c2e3222765b5b5fab6903f16ca3954cf62))
* support iperf3 authentication in server mode ([a1a7bc6](https://github.com/KISSMonX/LinkGauge/commit/a1a7bc619e4d7b88b5a6cbc2eefd099bf86121a4))
* use the new app icon in titlebar and about dialog ([a609082](https://github.com/KISSMonX/LinkGauge/commit/a609082f63000c09e95f0cc6495f1bac70bbb884))


### Bug Fixes

* default all 13 test items to enabled ([b7415e0](https://github.com/KISSMonX/LinkGauge/commit/b7415e05c4a0cb2e522cc2aaf206f112c306a416))
* dual-match client events and add dropped-event diagnostics ([630f205](https://github.com/KISSMonX/LinkGauge/commit/630f2051bb436a393e6c006f0c663c555e9a6222))
* explicitly set the window icon to the new icon set ([9183474](https://github.com/KISSMonX/LinkGauge/commit/9183474b1d7342d9301be238f92923002feb839f))
* failed queue items no longer abort the queue; stabilize queue rows ([c0b1df1](https://github.com/KISSMonX/LinkGauge/commit/c0b1df1c9661fe1e6e564c537eec547a2c05b1c2))
* **log:** persist frontend test errors to run logs ([1b9d4dc](https://github.com/KISSMonX/LinkGauge/commit/1b9d4dceaaa34e070ea63cd2e5b78570ebf58f1d))
* **logs:** 引擎日志移出常规同步包，各窗口由事件广播自建，消除日志冻结 ([40fea1a](https://github.com/KISSMonX/LinkGauge/commit/40fea1a14780e2e781f19975a5ed64e1304105d9))
* **log:** 日志文件逐行换行，与界面日志一致 ([48b37aa](https://github.com/KISSMonX/LinkGauge/commit/48b37aa27022905780b0b0205508e08c834206b4))
* **log:** 服务端日志文件名随界面语言（服务端 / Server） ([fa041f4](https://github.com/KISSMonX/LinkGauge/commit/fa041f478c44a457096f3056b64f682b7e714c06))
* **log:** 运行中的日志文件后缀随界面语言（进行中 / in progress） ([d6fca9f](https://github.com/KISSMonX/LinkGauge/commit/d6fca9fb5681ebe2ef70d571f0605d9be2dcadd9))
* migrate stored transferAmount 0 to 100 ([f81f608](https://github.com/KISSMonX/LinkGauge/commit/f81f6085c56db2fb792978d258f028ca962b38b7))
* primary button hover keeps blue gradient ([b6f6e9f](https://github.com/KISSMonX/LinkGauge/commit/b6f6e9f58c227fb70eb1d6983e1771ebc6384288))
* queue stalls after fast tasks (ping then byte-limited) ([06ea53a](https://github.com/KISSMonX/LinkGauge/commit/06ea53a70c3976ca8eea5dafc70a1ad326476c30))
* **queue:** enforce a hard watchdog deadline and advance after stalls ([1fc5687](https://github.com/KISSMonX/LinkGauge/commit/1fc5687a5bceb3245cc11278caadd3d920413c3e))
* **queue:** make stall timeout absolute via armed-index guards ([5b2564a](https://github.com/KISSMonX/LinkGauge/commit/5b2564a6b59e79c44cf89f9113f1478146428316))
* **queue:** prevent test stalls and stale UI state ([4da318f](https://github.com/KISSMonX/LinkGauge/commit/4da318f3b9dc5b0c48bd4ca7b27352e61b193352))
* **queue:** stop retrying failed tests ([f8b7656](https://github.com/KISSMonX/LinkGauge/commit/f8b7656f9c9d10df5bf741fd14f4d8f0da00892d))
* **queue:** 服务端不可达时中止整个队列，不再逐项空跑 ([0ed9e5f](https://github.com/KISSMonX/LinkGauge/commit/0ed9e5fd461ce9b64051e35cb8efb5eafd6fec3c))
* **queue:** 看门狗前置武装，消除 invoke 挂起导致的队列静默卡死 ([7bf7344](https://github.com/KISSMonX/LinkGauge/commit/7bf7344db52b3ae7200094474f33d6b247e648df))
* quiet late events from the previous queue item ([21523f9](https://github.com/KISSMonX/LinkGauge/commit/21523f9d849e358800c8f9b9c4f11d1c43371f54))
* reject duplicate server starts (server singleton) ([358ede8](https://github.com/KISSMonX/LinkGauge/commit/358ede88dd3e81d76540a58f92b4dc68d0299dda))
* revert invalid window icon config field ([39e9bba](https://github.com/KISSMonX/LinkGauge/commit/39e9bba5006c26f8498b8a11f061ac620ff8b8f5))
* **runtime:** recover server state and chart samples ([51b0792](https://github.com/KISSMonX/LinkGauge/commit/51b07928634485c53f29da863f97da0e4c1c4753))
* server start failed with 'test end condition must be time/bytes/blocks' ([ae63802](https://github.com/KISSMonX/LinkGauge/commit/ae63802c0cce0805a53a2154bb4be3e11731d048))
* **startup:** use native Windows network APIs ([c94ea81](https://github.com/KISSMonX/LinkGauge/commit/c94ea818ae27edcfdd04d0c2009a9e328c354846))
* **sync:** 队列推进状态只采纳驱动窗口的同步，消除随机静默卡死 ([8d1e6a8](https://github.com/KISSMonX/LinkGauge/commit/8d1e6a8c386438c0182323e41f2ab965e11c63d0))
* **sync:** 队列运行中的展示状态只随驱动窗口同步，高亮不再回退 ([d98d768](https://github.com/KISSMonX/LinkGauge/commit/d98d768e1a587fbd43c0caff63faa1340c905cc6))
* **ui:** disable unsupported test options and show explicit errors ([6dc6fb9](https://github.com/KISSMonX/LinkGauge/commit/6dc6fb96ca4ab612f1eb65fbe8973fae5cd7ead8))
* **ui:** enforce platform-specific test options ([4f1a5f5](https://github.com/KISSMonX/LinkGauge/commit/4f1a5f526df67a9d4d8d44d31718cc22a31ba04f))
* **ui:** 每项完成输出客户端日志，进度条改为当前项语义 ([3add29b](https://github.com/KISSMonX/LinkGauge/commit/3add29bcb43455b8bb1adf1ddbb53d3e5bffb6cb))
* watchdog advances the queue when task events are lost ([ac1d805](https://github.com/KISSMonX/LinkGauge/commit/ac1d805aa0df7373104a0ff0f9f6abd5164dd827))


### Miscellaneous Chores

* release 0.2.1 ([3835734](https://github.com/KISSMonX/LinkGauge/commit/38357343b4dbb1fd62068e3bd1fa935ba6d5da2a))
