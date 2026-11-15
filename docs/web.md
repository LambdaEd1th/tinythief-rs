# 浏览器（WASM）宿主说明

`crates/tt-web` 把宿主编译成 `wasm32-unknown-unknown`，用 `trunk` 打包，
Ruffle 通过 wgpu 的 **WebGL** 后端渲染到 `<canvas>`。原生宿主与 web 宿主共用
`tt-bridge`（ExternalInterface / 免费内购）与 `tt-assets`（AES 解密）。

## 数据流

```
页面加载
  → trunk 注入的 wasm-bindgen 脚本 init()
  → main.js 等 window.wasmBindings 后调用 TinyThiefHost.create(canvas, "assets/")
      ├─ WgpuRenderBackend::for_canvas(canvas, webgl)      // vendored Ruffle
      ├─ WebNavigator.load_movie("TinyThief.swf")          // fetch + AES 解密
      ├─ IdbStorage (IndexedDB, localStorage fallback)      // SharedObject 持久化
      ├─ WebAudioBackend                                    // 双 AudioBuffer 轮转播放
      ├─ WebLocale                                         // 时间/时区来自 JS Date
      └─ PlayerBuilder + tt_bridge::Bridge                 // 4 个原生接口 + 免费内购
  → requestAnimationFrame 循环：host.tick(dt)
      ├─ player.tick(dt)
      ├─ drain_outbox() → player.call_root_method(...)     // 购买结果/商品信息回传 AS3
      └─ player.render() → WebGL present
```

## wasm 上踩过的坑（已在代码/vendor 补丁中解决）

| 现象 | 原因 | 处理 |
|---|---|---|
| `time not implemented on this platform` panic | `chrono::Utc::now()`、`std::time::SystemTime::now()` 在 `wasm32-unknown-unknown` 不可用 | vendor 里加 `crate::now_utc()`（wasm 用 `js_sys::Date`）；`tt-platform` 的 NTP 同样走 JS 时钟 |
| AS3 的 `new Date()` 崩溃 | `DefaultLocaleBackend` 用 chrono | web 宿主传 `WebLocale`（JS `Date` 实现 `LocaleBackend`） |
| `getrandom` 报 wasm 不支持 | getrandom 0.4 需要显式后端 | `getrandom = { features = ["wasm_js"] }` |
| `linkme::distributed_slice` 编译失败 | `known_stubs` feature 不支持 wasm | 原生/web 都关闭 `known_stubs`（仅影响 stub 分类报告） |
| wasm-bindgen 版本不匹配 | crate 与 CLI 必须同版本 | 固定 `wasm-bindgen = "=0.2.128"`，CLI 放在 `tools/bin/` |
| trunk 报 `invalid value '1' for '--no-color'` | 环境里 `NO_COLOR=1` | 构建脚本用 `env -u NO_COLOR` |
| 画布全黑 | `for_canvas` 用 1×1 建成 swap chain | 建完 player 后显式 `set_viewport_dimensions(1024, 768, 1.0)` |
| 存档读不出来 | 每个 key 单独开事务并在 `await` 前丢掉事务句柄，Chrome 会卡住 | 一个只读事务里同时发 `getAllKeys` + `getAll`，再逐个配对 |

## 存档后端（IndexedDB）

Ruffle 的 `StorageBackend` 是**同步** trait，IndexedDB 是异步 API，所以：

1. 启动时打开数据库（首次用 `onupgradeneeded` 建 `sharedobjects` 表），
   用一个只读事务把全部键值载入内存 `HashMap`；
2. `get` 直接读内存；`put`/`remove_key` 先更新内存，再 `spawn_local` 一个异步写事务；
3. IndexedDB 不可用（如部分隐私模式）时回退 `localStorage`；两种后端**互不迁移**，
   旧 `localStorage` 存档不会被导入 IndexedDB。

验证（同 profile：先跑到游戏写入存档，再手动塞一个 `tinythief:TinyThief.swf/legacy`
到 localStorage，刷新后重载）：

```
indexeddb_keys_after_reload = 3   # 仍是游戏自己写的 3 个，legacy 未被导入
```

### 为什么不能用 `--screenshot` 验证

Chrome headless 的 `--virtual-time-budget` **不会派发 IndexedDB 事件**（纯 JS 探针也能复现），
页面会卡在 `Loading Tiny Thief…`。验证要么用真实时间的 CDP 脚本，要么手动开浏览器：

```bash
pip install websocket-client
python3 scripts/web_smoke.py --url http://127.0.0.1:8080/ \
    --profile /tmp/tt-web-prof --seconds 25 --expect-keys 3
```

同一个 `--profile` 跑第二次会带出上次写入的存档（`indexeddb_keys=3`），
这就是持久化的直接证据。

## 音频

Web 端用 `crates/tt-web/src/audio.rs`（移植自 Ruffle `web/src/audio.rs`）：

- 双 `AudioBufferSourceNode` 轮转，`onended` 时重新混音并排期；自适应缓冲
  （1024–16384 帧）沿用上游逻辑；
- **懒初始化**：`AudioContext` 不在启动时创建，而是在第一次 `pointerdown`/`keydown`
  时由 `host.resume_audio()` → `Player::unlock_audio()` → 后端 `unlock()` 同步创建。
  Chrome 允许"先创建、后 resume"，但 **Safari 只允许在手势处理器里创建的
  AudioContext 播放**，所以这是 Safari 能出声的关键；
- 若 `new AudioContext({sampleRate: 44100})` 不被支持（旧 Safari），自动退回默认采样率；
  混音缓冲始终以 44.1 kHz 创建，由浏览器负责重采样到设备率；
- 写回声道用 `getChannelData`（通过 `Reflect` 拿活的 `Float32Array`）而不是
  `copyToChannel`，兼容旧 WebKit；
- 首次点击前 `AudioContext.state === "not-started"`，之后变 `"running"`；
- 调试钩子：`__ttAudio()` 返回 `{state, time, peak}`，`peak` 是上次调用以来
  混音缓冲的最大绝对值（0 表示静音），可用于无头验证；
- `?mute=1` 可切回 `NullAudioBackend`。

实测（headless Chrome + SwiftShader，教学关）：开音频 58.1 fps / 静音 56.5 fps，
在噪声范围内；`peak` 在菜单/关卡里均为非零，说明确实在混音。

Safari 路径用 Playwright WebKit 27.2（UA：`Version/27.2 Safari/605.1.15`）验证：

```
BEFORE {"state":"not-started","time":0,"peak":0}
AFTER1 {"state":"running","time":2.98,"peak":0.0227}
AFTER2 {"state":"running","time":5.48,"peak":0.0763}   ← 手势后创建 context 并出声
```

顺带说明：控制台里的 `Failed to parse loader URL when extracting query parameters`
是 Ruffle 对相对 URL 的既有告警（与音频无关）。

## 加载时间

web 比 native 慢的部分几乎全是**网络传输**，不是解密或渲染：

| 阶段 | 冷启动（Pages） | 热启动（HTTP 缓存） |
|---|---|---|
| HTML/JS | ~0.3 s | ~0.03 s |
| wasm（gzip 3.6 MB / 原始 9.2 MB） | 0.35 s | 0.02 s |
| 启动 SWF（2.0 MB） | 0.19 s | 0.002 s |
| host 初始化（WebGL + 解密 + 解析 + IndexedDB） | ~0.2 s | ~0.2 s |
| 游戏内开屏动画（24 fps） | ~4.5 s | ~4.5 s |

native 侧同一阶段实测：renderer 39 ms、启动 SWF 读取+解密 28 ms、解析 9 ms、
player ready 176 ms——所以宿主初始化只比 web 快 ~30 ms，剩下全是下载时间。

优化一：页面会以低优先级在后台预取稍后才用的资源（splash/menu/theme 等约 3.6 MB），
把下载藏进开屏动画里；`?prefetch=0` 或浏览器 Save-Data 时关闭。
5 Mbps 限速实测：菜单就绪从 29.8 s 降到 27.7 s。

优化二：Service Worker（`crates/tt-web/sw.js`）缓存 shell（index/main.js/带哈希的
wasm）和所有 `assets/` 请求。缓存名由 wasm 内容哈希派生，新版本部署后自动建新缓存、
删旧缓存：

- 导航：network-first，失败回退缓存（离线可启动）；
- `main.js`：stale-while-revalidate；
- `assets/*` 与哈希化的 js/wasm：cache-first。

实测（5 Mbps 限速 + 清空 HTTP 缓存，同一 profile 二次访问）：

```
no-sw   menu.swf=27613ms   wasm=18482ms
sw      menu.swf= 5042ms   wasm=   26ms   ← 全部来自 Cache Storage
```

离线（CDP offline）二次启动同样能到菜单（`player ready=True`，菜单 5.0 s）。
`?sw=0` 会注销 Service Worker，便于调试缓存。

## 性能

### 测量（Chrome headless + SwiftShader 软件 WebGL，教学关，12 秒窗口）

| 配置 | tick/窗口 | render/窗口 | render 占 tick | CPU | p50 帧时 | fps |
|---|---|---|---|---|---|---|
| 默认（按需渲染 + quality=high） | 718 | 289 | 40% | 57.3% | 16.7 ms | 59.6 |
| `?alwaysRender=1`（旧行为） | 268 | 268 | 100% | 101% | 50.0 ms | 22.2 |
| 默认 + `?quality=low` | 722 | 288 | 40% | 33.9% | 16.7 ms | 60.0 |

结论：**按需渲染是最大头**——旧行为在软件 WebGL 下把 CPU 打满、掉到 22 fps；
`quality=low` 再省 ~40% CPU（矢量抗锯齿关闭），给重关卡留出余量。

### 已做的优化

1. **按需渲染**：`Player::needs_render()` 只在 SWF 真正推进一帧或输入改变显示列表时为真；
   SWF 是 24 fps，rAF 是 ~60 fps，跳过 60% 的渲染。
2. **画质档位**：`?quality=low|medium|high|best`（原生 `--quality`），
   `low` 关闭矢量抗锯齿，软件/低端 GPU 上收益最大。
3. **wasm 构建**：`-C target-feature=+simd128` + `lto="thin"` + `codegen-units=1`，
   wasm 从 9.7 MB 降到 9.0 MB。
4. **指针事件合并**：`pointermove` 只记最后位置，每个渲染帧最多处理一次 hover。
5. **原生 dev profile**：`opt-level=1`（依赖 `opt-level=2`），
   `cargo run` 直接玩不再卡成幻灯片；发布用 `--release`。

### 还能做

- **关卡预取**：进入关卡时预取相邻关卡 SWF（fetch+解密），减少切换时的首帧卡顿；
- **纹理缓存上限**：重关卡贴图换入换出时可调大 Ruffle 的纹理缓存（需要看它的渲染配置）；
- **帧率上限**：把 `tick` 也压到 24 Hz（目前 tick 60 Hz 只做计时/音频尾巴，开销小）。

## 高帧率评估

**结论：不能真正上高帧率。** 这不是引擎限制，是内容设计——整个游戏是 24 fps 逐帧逻辑。

证据（FFDec 导出的 AS3）：

| 事实 | 位置 |
|---|---|
| SWF 元数据写死 `frameRate="24"` | `Main.as` |
| 游戏循环 `addEventListener(Event.ENTER_FRAME, internalUpdate)` | `J2DM_AbstractGameLoop.as` |
| `update()` 不带 delta，movement 是 `x += 固定步长` | `J2DM_AbstractGameLoop` / `J2DM_CharacterController` |
| 全工程搜不到 `deltaTime` / `elapsedTime` | 0 个文件 |
| 225 个文件用时间轴 `gotoAndStop/Play`，58 个用 Greensock 补间 | AS3 统计 |
| 没有任何地方改 `stage.frameRate` | 只有 `Main.as` 元数据 |

所以把帧率调高 = **整体 2.5× 加速**（不是变流畅）。实测 `?frameRate=60`：

```
24 fps: 开屏到主菜单 5.41 s
60 fps: 开屏到主菜单 3.21 s
```

扣掉固定加载时间（~1.7 s）后，开屏动画从 3.67 s 变成 1.47 s，正好 2.5×。
这个开关只适合做 speed-run/调试，不能当"高帧率模式"。

### 为什么 24 fps 在 60 Hz 屏上本来就"看起来掉帧"

24 不能整除 60：每个 SWF 帧要跨 2.5 个 rAF tick，实际呈现间隔在 33 ms / 50 ms 之间交替，
周期 5 个 tick。这不是性能问题，是 24 fps 内容在 60 Hz 刷新率上的固有抖动
（120 Hz 屏上是 5 tick 整除，反而均匀）。想消除只能做**渲染插值**。

### 如果一定要高帧率

唯一忠实方案是 Ruffle fork 里的**渲染插值**：逻辑仍跑 24 Hz，渲染时对显示对象的
transform（位置/旋转/缩放/alpha）在上两帧之间插值。代价：

- 要改渲染路径和显示对象 transform 管线（嵌套 clip、mask、filter、scrollRect 都要考虑）；
- 走路循环等时间轴动画仍是 24 Hz，收益主要在镜头平移和物体移动；
- 渲染成本 ×2.5，低端设备反而更容易掉帧。

工作量是周级，收益有限，不建议现在做。更实际的是把 24 fps 的**帧时间做稳**：
按需渲染（已做）、关卡预取、减少 GC 峰值。

## 本地验证

```bash
scripts/prepare_web_assets.sh
scripts/build_web.sh
cd crates/tt-web/dist && python3 -m http.server 8765

# 无头截图（Chrome + SwiftShader 软件 WebGL）
"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
  --headless=new --disable-gpu --enable-unsafe-swiftshader --no-sandbox \
  --window-size=1100,900 --virtual-time-budget=60000 \
  --screenshot=/tmp/web.png "http://127.0.0.1:8765/?warmup=800"
```

`?warmup=800` 会在首帧前跑 800 帧（约 26 秒游戏时间），这样截图就是主菜单而不是
Rovio Stars 开屏。也可以加 `--enable-logging=stderr --v=0` 抓 `tracing` 日志。

## 发布到 GitHub Pages

`.github/workflows/pages.yml`：

1. checkout + `wasm32-unknown-unknown` + `trunk` + `wasm-bindgen-cli 0.2.128`；
2. `trunk build --release --public-url "/${{ github.event.repository.name }}/"`；
3. 上传 `crates/tt-web/dist` 并 `deploy-pages`。

资产（59 MB）默认在 `.gitignore` 里。要发布完整可玩的站点，需要把
`assets/` 目录提交进仓库（或改成从带 CORS 的对象存储加载，
页面支持 `?assets=<base-url>`）。版权归 Rovio，公开部署前请确认授权。

## 后续可做

1. 音频方面可做：主音量/静音按钮、`AudioContext` 设备切换（`setSinkId`）。
2. 首屏 Loading 进度条（wasm 9.7 MB + 启动 SWF 2 MB）。
3. 用 `?warmup` + 截图做 web 端的 golden-image 回归。
