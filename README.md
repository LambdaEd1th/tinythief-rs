# tinythief-rs — Tiny Thief 跨平台宿主（方案 B）

[![CI](https://github.com/LambdaEd1th/tinythief-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/LambdaEd1th/tinythief-rs/actions/workflows/ci.yml)
[![Pages](https://github.com/LambdaEd1th/tinythief-rs/actions/workflows/pages.yml/badge.svg)](https://github.com/LambdaEd1th/tinythief-rs/actions/workflows/pages.yml)
[![Release](https://github.com/LambdaEd1th/tinythief-rs/actions/workflows/release.yml/badge.svg)](https://github.com/LambdaEd1th/tinythief-rs/actions/workflows/release.yml)

用 **Rust + wgpu + Ruffle** 复刻 Tiny Thief 的 iOS 原生宿主：Ruffle 提供 AS3/AVM2 + SWF 渲染，
本工程提供宿主层（解密资产、原生接口桥、存档、输入、平台服务）。同一套代码编译到
native（winit + wgpu）和 WebAssembly（trunk + WebGL）。

> 在线试玩：<https://lambdaed1th.github.io/tinythief-rs/>
> 背景与可行性分析见 [`docs/rust-host-feasibility.md`](docs/rust-host-feasibility.md)，
> 二进制逆向细节见 [`docs/TinyThief-architecture.md`](docs/TinyThief-architecture.md)。

> **版权声明**：Tiny Thief 的代码与美术资源版权归 Rovio Entertainment 所有。
> 本仓库仅用于技术研究与个人备份，仓库内资产**不是**自由软件；如需商用或公开分发请自行获得授权。
> 详见 [`NOTICE.md`](NOTICE.md)。

## 当前状态

| 里程碑 | 内容 | 状态 |
|---|---|---|
| **M0** | 运行时 AES-256-CBC/PKCS7 解密资产 Provider（不落地明文） | ✅ 完成，`tt-assets` 5 个测试通过 |
| **M1** | 窗口 + wgpu + Ruffle player + 启动到主菜单 | ✅ 完成 |
| **M2** | 原生接口桥 / 音频 / 存档 / 输入 | 🟡 大部分完成（见下表） |
| M3 | 保真度长尾（文本/滤镜/时序）+ 真机平台服务 | ⬜ 未开始 |

M2 明细：

| 项 | 状态 | 证据 |
|---|---|---|
| 4 接口 29 函数桥接 | ✅ | `bridge.rs`，实跑覆盖 init/retrieveProductsInfo/logEvent/getPlatformID/getNTPTime/loginUser |
| 参数编组（List/Object） | ✅ | 7 个 IAP 商品 ID、Flurry 事件元数据均正确解析 |
| 音频 | ✅ | cpal 后端 + MP3 解码（`sounds/menu/*.mp3` 正常加载） |
| 存档（SharedObject） | ✅ | 落盘 `.sol`，二次运行 `Session_ID` 自增证明读取成功 |
| 输入映射 | ✅ | 脚本化三连点击走完 菜单→开场漫画→任务卡→**关卡实机**，点地可让主角行走 |
| 生命周期钩子 | ✅ | 失焦/`suspended()` 时调用 AS3 `onPaused()` 并冻结，恢复时 `onResumed()`；`--pause-after` 可复现冻结 |
| 免费内购 | ✅ | 商品一律报 `Free`；点 Buy 后宿主直接回调 `IAPInterfaceOnPaymentCompleted(id, true)`，游戏按正常购买流程解锁并写存档 |
| 平台服务 | ✅（空实现） | `Platform::null()`：29 个函数全部接线并打日志，后端一律 no-op、不落盘 |

M1 实测（无头截图，`docs/images/host_m1_menu.png`）：

```bash
cargo run -p tt-host -- ../TinyThief.app TinyThief.swf \
    --capture /tmp/menu.png --frames 300
```

启动链路（宿主日志实测）：

```
TinyThief.swf
  → swf/logosState.swf
  → config/sounds/Fx_config.xml + Animations_config.xml
  → swf/commonGamePlay.swf
  → swf/levelTool.swf
  → swf/splashScreen.swf → swf/windowGift.swf → swf/menu.swf
  → ExternalInterface: FlurryInterface.init / GameServicesInterface.init /
    IAPInterface.init / retrieveProductsInfo / GameServicesInterface.loginUser
```

## 工程结构

```
tinythief-rs/
├── crates/
│   ├── tt-assets/     # AES-256-CBC + PKCS7 解密、AppBundle 虚拟文件系统（无 ruffle 依赖）
│   ├── tt-platform/   # PlatformServices：4 个接口 29 个函数的 trait + **空实现**
│   ├── tt-bridge/     # ExternalInterface 桥（原生与 web 共用：4 接口 + 免费内购）
│   ├── tt-host/       # 原生宿主二进制（winit + wgpu）
│   ├── tt-web/        # 浏览器宿主（wasm-bindgen + WebGL + trunk）
│   └── vendor/ruffle/ # vendor 的 Ruffle 子集 + host→AS3 补丁（见 docs/ruffle-patch.md）
│       ├── main.rs      # CLI：窗口模式 / --capture 无头模式
│       ├── host.rs      # winit + wgpu + ruffle Player 驱动、输入映射、离屏截图
│       ├── navigator.rs # NavigatorBackend：把 URL 映射到 AppBundle 并按需解密
│       └── bridge.rs    # ExternalInterface/FsCommand → tt-platform
└── Cargo.toml         # workspace；Ruffle 以 git tag 锁定
```

关键依赖：

```toml
ruffle_core       = { git = "https://github.com/ruffle-rs/ruffle", tag = "v0.7.1", ... }
ruffle_render_wgpu = { git = "https://github.com/ruffle-rs/ruffle", tag = "v0.7.1" }
```

> `ruffle_core` 未发布到 crates.io，只能 git 依赖；升级需重跑验收。

## 如何运行

### 前置条件

| 项 | 要求 |
|---|---|
| 工具链 | Rust ≥ 1.85（实测 1.100） |
| 图形 | macOS（Metal）/ Windows（DX12）/ Linux（Vulkan） |
| 内容 | 原始的 `TinyThief.app`（**加密的原包**，宿主运行时自行解密，不需要先解出 `decrypted/`） |
| 音频 | 可选，没有输出设备会自动降级为静音 |

首次构建要编译 vendored Ruffle（约 13 MB 源码），debug 构建约 2–4 分钟；
之后增量构建几秒。

### 两个入口

```bash
cd tinythief-rs

# 1) 窗口模式：跑到主菜单，1024x768，Esc 退出
cargo run -p tt-host -- ../TinyThief.app

# 2) 无头模式：渲染 N 帧后截图退出（CI/对比用）
cargo run -p tt-host -- ../TinyThief.app TinyThief.swf \
    --capture /tmp/menu.png --frames 600
```

从仓库根目录跑也可以：

```bash
cargo run --manifest-path tinythief-rs/Cargo.toml -p tt-host -- tinythief/TinyThief.app
```

> 第 1 个参数是 `TinyThief.app` 路径，第 2 个（可选）是启动影片，默认 `TinyThief.swf`，
> 也可以传 `swf/menu.swf` 之类的资源做单独调试。

### 操作

- 鼠标 = 触摸：点按钮、点地面让主角走；
- 窗口失焦/最小化会调用 AS3 的 `onPaused()` 并冻结，恢复时 `onResumed()`；
- 关窗或 Esc 退出。

### 内购

所有内购商品都显示 **Free**，点 Buy 直接完成交易并按正常流程解锁章节（见下文"免费内购"）。

### 存档与日志

```
saves/TinyThief.swf/*.sol     # SharedObject 存档（TT_SAVE_DIR 可改目录）
```

> 内容不会默认解锁：全新存档只有 `MonetizationMode` / `FreemiumUser` 两个默认键，
> 没有任何 `historyN` 或 `NumberPremiumHints`（`verify.sh` 有断言）。
> 如果你跑过 15:5x 那个短暂版本的"预解锁"构建，`saves/` 里可能残留全解锁存档，
> 删掉 `saves/` 即可回到正常进度。

- `RUST_LOG=info,tt_host=debug` 打开宿主/IAP/资源日志；
- `TT_NO_AUDIO=1` 关闭音频；
- 想看存档内容：`cargo run -p tt-host --example dump_sol -- saves/TinyThief.swf/tinyThiefDataNew.sol`
- 想强制走 release（更流畅）：`cargo run --release -p tt-host -- ../TinyThief.app`（首次编译较久）。

### 一次性验收

```bash
./scripts/verify.sh            # 构建 + 单测 + 主菜单 + 进关卡 + 免费内购 + 存档 + 暂停 + 确定性
```

## Web / GitHub Pages

浏览器版是把同一套 `tt-bridge` + vendored Ruffle + 解密资产管线编译到
`wasm32-unknown-unknown`，用 WebGL 渲染到 `<canvas>`。

![Web 主菜单](docs/images/web_menu.png)

### 构建

```bash
# 1) 把加密的原包放进 web crate（运行时在浏览器内存里解密，不需要 decrypted/）
scripts/prepare_web_assets.sh            # -> crates/tt-web/assets  (~59 MB)

# 2) 构建（GitHub Pages 项目站点必须带 --public-url "/<仓库名>/"）
scripts/build_web.sh --public-url /tinythief-rs/
#    产物：crates/tt-web/dist  (~69 MB = 9.7 MB wasm + 59 MB assets)

# 3) 本地预览
cd crates/tt-web/dist && python3 -m http.server 8080
#    http://127.0.0.1:8080/
```

> `--public-url` 不传时资源用绝对路径 `/`，只适合本地预览或用户主页站点；
> 项目站点（`https://<user>.github.io/<repo>/`）必须传 `/<repo>/`。

### 存档

浏览器版把 SharedObject 存在 IndexedDB（`tinythief` 库、`sharedobjects` 表）。
Ruffle 的 `StorageBackend` 是同步接口，而 IndexedDB 是异步的，所以实现方式是
**启动时一次性载入内存 → 读写走内存 → 写操作异步写穿到 IndexedDB**。
`localStorage` 的旧存档**不会**导入：IndexedDB 从空库开始。

### Web 冒烟测试

```bash
pip install websocket-client
python3 scripts/web_smoke.py --url http://127.0.0.1:8080/ \
    --profile /tmp/tt-web-prof --seconds 25 --expect-keys 3
# player_ready=True panic=False indexeddb_keys=3
```

注意：Chrome 的 `--virtual-time-budget` **不会**派发 IndexedDB 回调，
所以截图/自动化要用真实时间的 CDP 驱动（就是上面这个脚本），
`?warmup=N` 只适合不需要 IndexedDB 的快速截图。

### 调试参数

| 参数 | 作用 |
|---|---|
| `?warmup=N` | 首帧前先跑 N 帧（用于截图/冒烟测试，等价原生的 `--frames`） |
| `?quality=low\|medium\|high` | 画质档位；`low` 关闭矢量抗锯齿，低端设备省 ~40% CPU |
| `?assets=<url>` | 资产改从别处加载（需 CORS），便于把 59 MB 资源放独立存储 |
| `?alwaysRender=1` | 关掉按需渲染（A/B 对比用，不要用于发布） |
| `?frameRate=N` | 覆盖 SWF 帧率；会让游戏整体加速 N/24 倍（仅调试/speed-run 用，不是高帧率模式） |

### GitHub Pages

`.github/workflows/pages.yml` 已就绪：推 `main` 后自动构建并发布 `crates/tt-web/dist`。
注意资产目录默认被 `.gitignore` 忽略（版权原因），CI 里要使用需要**自行决定**是否提交
`crates/tt-web/assets/`（工作流里对应步骤已注释）。

### 性能

默认按需渲染（SWF 24 fps，rAF 60 fps，只渲染真正推进的帧）。软件 WebGL 实测：

| 配置 | CPU | p50 帧时 | fps |
|---|---|---|---|
| 默认 | 57% | 16.7 ms | 59.6 |
| `?alwaysRender=1`（旧行为） | 101% | 50 ms | 22.2 |
| 默认 + `?quality=low` | 34% | 16.7 ms | 60 |

原生宿主同理支持 `--quality low|medium|high`、`--frame-rate N`；`cargo run` 现在用 `opt-level=1`
（依赖 `opt-level=2`），可玩性接近 release。

### 已知限制

| 项 | 现状 |
|---|---|
| 音频 | 浏览器版目前是静音（`NullAudioBackend`）；接 WebAudio 需移植 Ruffle 的 web 音频后端（约 280 行） |
| 存档 | **IndexedDB**（库 `tinythief` / 表 `sharedobjects`）；不支持时回退 `localStorage`。不做旧存档迁移 |
| 触摸手势 | 与原生一致：单点/拖拽可用，多点手势 Ruffle 无对应事件 |
| 包体 | 首次加载 9.7 MB wasm + 2 MB 启动 SWF，其余资源按需拉取 |

> 版权提醒：资产是 Rovio 的受版权内容，公开部署到 Pages 属于再分发，请自行确认授权。

## 命令行参数

| 参数 | 说明 |
|---|---|
| `<app>`（位置 1） | `TinyThief.app` 路径 |
| `<movie>`（位置 2） | 启动影片，默认 `TinyThief.swf`（可换 `swf/menu.swf` 调试） |
| `--capture <png>` | 无头模式：渲染后截图退出 |
| `--frames N` | 无头模式渲染帧数（默认 300，30fps） |
| `--click X,Y[@帧]` | 注入点击（可重复；按下/抬起相隔 4 帧）。例：`--click 512,660@640` |
| `--snapshot 帧:路径` | 在指定帧额外保存一张截图（可重复，用于对比） |
| `--pause-after N` `--pause-frames M` | 模拟生命周期暂停：N..N+M 帧不 tick |
| `--click-frame N` | 未写 `@帧` 时的默认帧号（默认 240） |

环境变量：`TT_SAVE_DIR`（存档目录，默认 `./saves`）、`TT_NO_AUDIO=1`（禁用音频）、`RUST_LOG`。

持久化文件只有存档：

```
saves/TinyThief.swf/*.sol   # SharedObject（路径已规范化，不含绝对路径）
```

> 平台服务按设计是**空实现**：`Platform::null()` 下 `GameServices` 登录返回失败、
`IAP` 只记日志、`NativeUtils.getNTPTime` 回退系统时钟、`Flurry` 仅 `debug!` 输出。
游戏仍可正常启动并进入关卡（已实测，与 Ruffle 的 `--dummy-external-interface` 行为一致）。

### 免费内购是怎么做的

内购不走平台后端，而是由**宿主直接完成交易**：

1. AS3 调 `IAPInterface.retrieveProductsInfo([7 个商品])`
   → 宿主回 `IAPInterfaceOnProductInfoReceived(ids, [0.0…], ["Free"…])`，
   商店因此显示 `Free`；
2. 玩家点 Buy → AS3 调 `IAPInterface.requestPayment(id)`
   → 宿主立刻调 `IAPInterfaceOnPaymentCompleted(id, true)`，
   AS3 的 `IAPHelper.onPaymentComplete` 走正常购买分支，解锁对应章节并写入存档；
3. `restoreCompletedPurchases` → 宿主回 `IAPInterfaceOnRestorationCompleted(true)`。

这套回调需要宿主能主动调用 AS3，而 Ruffle 没有暴露该能力，
因此 vendor 了 Ruffle 并加了一个 `Player::call_root_method`（约 70 行），
详见 [`docs/ruffle-patch.md`](docs/ruffle-patch.md)。

## 已知问题 / 下一步

0. **平台服务是空实现**（本轮按要求调整）：只保留 trait 抽象 + `Platform::null()`；
   真实后端（StoreKit / Game Center / Play Billing / Steamworks）后续从 trait 接入。
3. **触摸手势**：目前是鼠标→触摸的单点映射，swipe/pinch/rotate 未接。
4. **存档路径**：Ruffle 用 SWF URL 生成子目录（`saves/localhost/Volumes/...`），
   后续应做路径规范化。
5. **MP3 解码**：`ruffle_core` 必须以 `features = ["mp3", "audio"]` 构建，
   否则音效加载失败会让游戏卡在标题画面（本工程已开启）。
6. **debug 构建**：Ruffle AVM2 深度加法按 Flash 语义回绕，
   故 workspace 设置 `overflow-checks = false`。
7. **URL 解析**：游戏用相对路径（`swf/menu.swf`）请求资源，navigator 直接按
   bundle 根解析；Ruffle 会警告 "relative URL without a base"，属预期噪音。

## 设计取舍

- **不落地明文**：`AppBundle::read` 在内存里解密，navigator 直接把字节交给 Ruffle。
- **平台无关**：`tt-platform` 只定义 trait，iOS/Android/桌面的实现各自注入；
  没有平台服务时可用 `Platform::null()`，游戏仍能启动（已实测）。
- **可测试**：`--capture` 模式让每个界面都能在 CI 里产出基准图，作为保真度验收手段。
