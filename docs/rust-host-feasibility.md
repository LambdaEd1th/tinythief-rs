# 用纯 Rust / wgpu 复刻 Tiny Thief 跨平台宿主 —— 可行性评估

> 结论先行：**可行，但不是"从零写一个 Flash 运行时"**。
> 唯一现实的路线是：**用 Ruffle（Rust + wgpu）当 AS3/SWF 内核，再在 Rust 里写宿主层**。
> 而且这条路线已经被我实测验证过一半：**现成的 Ruffle 0.7.1 不做任何改动，就已经把 Tiny Thief 的主菜单渲染出来了**（见 §2 与 `evidence/ruffle_menu.png`）。

---

## 0. 给结论分级

| 目标 | 可行性 | 现实工作量（1–2 名熟练 Rust/图形工程师） |
|---|---|---|
| 在现代设备/桌面上**能跑、能玩通关**（平台功能可降级） | **高**，已有实证 | **2–4 人周** |
| **完整**复刻宿主（内购/成就/分享/统计/存档/暂停恢复/多平台） | **高**，工程量大但风险可控 | **3–5 人月** |
| **"精确"**（逻辑+存档一致、视觉近似、时序一致） | **中高** | 再加 **2–3 人月** 的长尾打磨 |
| **像素级精确**（bit-exact 画面） | **不现实** | 需要重写 Scaleform 光栅化器，不建议 |

一句话：**"完整跨平台"是工程问题；"精确"是保真度定义问题；"从零用 Rust 写 Flash 虚拟机"是人年问题，不建议。**

---

## 1. 先把"宿主"拆开：你真正要复刻的是什么

这个 6.15 MB 的 armv7 二进制，按代码占比可以粗分成三块：

| 组成 | 体量（估计） | 复刻难度 |
|---|---|---|
| **Scaleform GFx 4.x 运行时**（SWF 解析 + AVM2 虚拟机 + AS3 类库 + 矢量光栅化 + 文本/输入/IME） | `__text` 4.9 MB 的绝大部分，上万函数 | ⛔ 从零写 = 人年（Ruffle 本身就是多年、十万行级项目） |
| **FMOD 4（Rovio fork）** | 次大块（`FMOD::*` 导出数百个符号） | 🟢 可完全替换：本作音频是**明文 MP3**，不需要 FSB/FMOD 专有格式 |
| **游戏自身的原生胶水**（ObjC 外壳 + 4 个接口 + 收据校验 + Appirater + NTP） | 约 70–90 KB（集中在 `0x9780–0x1D300`，约占 `__text` 的 1.5%） | 🟢 纯业务逻辑，Rust 里重写很快 |

**关键判断**：所谓"复刻宿主"，90% 的成本不在宿主胶水，而在那个内嵌的 Flash/Scaleform 运行时。所以问题实际上变成：

> 有没有现成的、够精确的 AS3/SWF 运行时可以用？

答案是 **有**：Ruffle。

---

## 2. 实证：这不是纸上推演，我真的把它跑起来了

### 2.1 实验设置

- 引擎：**Ruffle 0.7.1**（rel. 2026-10-07，Rust 实现，渲染后端 `ruffle_render_wgpu`，macOS 走 **Metal**）
- 内容：我用上一轮逆向出的 AES 密钥解密的 88 个 SWF + 187 个 XML（`decrypted/`）
- 运行方式：`ruffle --no-gui --dummy-external-interface --graphics metal --storage memory TinyThief.swf`
  （`--dummy-external-interface` = 完全空的 ExternalInterface，即**宿主侧一个原生函数都没实现**）

### 2.2 结果一：AVM2 直接跑通，无脚本错误

修正 PKCS7 填充后，日志里 **没有任何 AVM2 异常**，游戏状态机正常推进：

```
INFO  ruffle_core::context: Loaded SWF version 14, resolution 1024x768 @ 24 FPS
WARN  ruffle_core::stub: Encountered stub: AVM2 flash.display.Loader.load() addChild at the correct time
WARN  ruffle_core::stub: Encountered stub: AVM2 flash.utils.Dictionary constructor with weak keys
WARN  ruffle_core::stub: Encountered stub: AVM2 flash.display.Loader.unloadAndStop()
WARN  ruffle_core::stub: Encountered stub: AVM2 flash.media.Sound.load() with context
```
（只有 5 个已知 stub 警告，0 个错误。完整日志：`evidence/ruffle_run.log`）

### 2.3 结果二：主菜单真的渲染出来了

![Ruffle 运行 Tiny Thief 主菜单](evidence/ruffle_menu.png)

窗口标题 `Ruffle - Tiny Thief.swf`，标题画面、Tiny Thief logo、底部三个圆形按钮（设置 / 开始 / 返回）都正常显示 —— **没有任何一行自定义宿主代码**。

### 2.4 结果三：缺口被精确定位成 9 个调用

`--dummy-external-interface` 让所有原生调用"落空"，正好把这个宿主必须实现的面完整打印出来：

```
FlurryInterface.init            GameServicesInterface.init       IAPInterface.init
NativeUtils.getPlatformID       NativeUtils.getNTPTime           NativeUtils.getDeviceLanguage
GameServicesInterface.loginUser IAPInterface.retrieveProductsInfo FlurryInterface.logEvent
```

这与上一轮 IDA 逆向出的"4 个接口 / 29 个函数"完全对得上 —— **宿主需要实现的东西是有界、可枚举的**。

### 2.5 结果四：顺带修掉了我上一轮报告里的一个错误

第一次跑的时候，游戏在 `LogosState` 里报：

```
TypeError: Error #1088: markup in the document following the root element must be well-formed.
    at sound.data::XmlFxConfig$/setData()
```

原因不是 Ruffle，而是我的解密脚本没剥 **PKCS7 填充**：XML 根元素后面多了 `\x01`（或 13 个 `\x0D`）。
修正后一路顺利。这也说明：**用 Ruffle 当"参考实现"来做资产管线校验，非常高效**（`tools/decrypt_assets.py` 已修）。

### 2.6 这个实证说明了什么

| 结论 | 含义 |
|---|---|
| 游戏逻辑（2.48 MB ABC、J2DM 框架）在非 Scaleform 运行时上可执行 | **不必**逆向 AS3 逻辑，也不必将逻辑重写成 Rust |
| 渲染能出画面且看起来是对的 | 视觉保真度起点很高，剩下的主要是文本/滤镜/AA 长尾 |
| 缺失面只有 9 个外部调用名 | 宿主层工作量**可控**，不是无底洞 |
| 音频是明文 MP3 | 不需要 FMOD，`cpal`/`rodio`/`symphonia` 即可 |

---

## 3. 三条路线对比

| | A. 从零自研 Flash/Scaleform 兼容运行时 (Rust) | **B. Ruffle 内核 + 自研 Rust 宿主（推荐）** | C. 抛弃 SWF，用 Rust 引擎重制玩法 |
|---|---|---|---|
| SWF/AVM2/AS3 类库 | 全部自己写 | 现成（Ruffle） | 不需要（但要重写 2.48 MB ABC 里的玩法） |
| 渲染 | 自己写光栅化器 | 现成（wgpu，已有 Metal/Vulkan/DX12/GL 后端） | 自己写（wgpu） |
| 内容复用 | 100% | 100%（解密后直接喂） | 需要导出全部美术/动画/关卡数据 |
| 保真度 | 取决于你写的质量 | 中高（见 §6） | 低（等于重制） |
| 估算 | **10–30 人年** | **3–6 人月** | **12+ 人月**（等于重做一款游戏） |
| 风险 | 极高 | 中（集中在 Ruffle 的 AVM2 边界情况） | 极高（玩法逻辑要从字节码逆推） |
| 结论 | ❌ 不可行 | ✅ **唯一现实路线** | ❌ 不是"复刻宿主" |

补充说明为什么 A 不可行：Ruffle 本身是多年、十万行级代码、多人协作的成果，且至今仍在补 AVM2 边界；
"纯 Rust 从零写一个能跑商业 AS3 游戏的运行时"不是项目排期能解决的问题。

---

## 4. 推荐方案：Rust 宿主 + Ruffle 内核

### 4.1 技术栈（全部 Rust）

```
┌─────────────────────────────── Rust 宿主（自研，约 5–8k 行）───────────────────────────────┐
│  main / winit 窗口  →  wgpu surface  →  ruffle_render_wgpu (Metal/Vulkan/DX12/GL)          │
│  AssetProvider (AES-256-CBC + PKCS7 剥离 + 包内虚拟路径)                                   │
│  ExternalInterfaceProvider (9 个 EI 名字 + 29 个函数)  ← 本方案的核心工作量                │
│  PlatformServices trait → StoreKit/Play Billing/Steamworks · GK/Play Games/Steam · 分享/统计 │
│  SaveStore (SharedObject 落盘) · Clock (NTP/防改表) · InputMap (触摸/鼠标/手柄)             │
│  BootStateMachine (_root.onPaused/onResumed · splash 移除 · 60fps 驱动)                     │
└──────────────────────────────────────────┬──────────────────────────────────────────────────┘
                                           │ ruffle_core::PlayerBuilder
┌──────────────────────────────────────────┴──────────────────────────────────────────────────┐
│ Ruffle (git 依赖，锁定 tag)  core / render_wgpu / audio(cpal) / swf / avm2                  │
└─────────────────────────────────────────────────────────────────────────────────────────────┘
```

### 4.2 关键集成点（已核对 v0.7.1 源码）

| 集成点 | API / 事实 | 说明 |
|---|---|---|
| 玩家实例 | `ruffle_core::PlayerBuilder` | 建 `Player`，传 `NullUiBackend` + 自定 `Navigator` |
| 资源加载 | `Navigator`（`open_url`） | 在这里做**解密 + 虚拟路径**，游戏永远看不到密文 |
| 原生接口 | `core::external::ExternalInterfaceProvider::call_method(ctx, name, args) -> Value` | 正好对应那 9 个名字 |
| fscommand | `FsCommandProvider` | 备用通道 |
| 音频 | Ruffle 自带音频后端（cpal） | 不需要 FMOD；MP3 明文可直接播 |
| 渲染 | `ruffle_render_wgpu` | macOS=Metal，Win=DX12，Linux=Vulkan，Web=WebGL/WebGPU |
| 许可 | Ruffle 为 MIT / Apache-2.0 双许可 | 商用/嵌入门槛低（仍建议核对仓库 LICENSE 与所含第三方许可） |
| crate 获取 | **`ruffle_core` 未发布到 crates.io** | 必须 `git = "https://github.com/ruffle-rs/ruffle", tag = "..."` 锁定版本 |

### 4.3 实测出来的一个硬约束（必须先解决）

Ruffle 的 `ExternalInterface::Value` 是：

```rust
pub enum Value { Undefined, Null, Bool(bool), Number(f64), String(String),
                 Object(BTreeMap<String, Value>), List(Vec<Value>) }
```

运行时会打印：

```
WARN ruffle_core::external: from_avm2 needs to be implemented for Avm2Value::Object
```

也就是说 **AVM2 的对象/函数无法桥接到宿主**。而 Tiny Thief 的 `GameServicesInterface.init` 恰好是
"传一个含回调函数的对象给原生"的经典模式（原生拿到回调后，异步（内购结果、登录结果）再回调 AS3）。

两条解法：

1. **给 Ruffle 打补丁**（推荐，约 1–3 人周）：给 `Value` 增加可回持句柄（如 `ObjectHandle(u32)`），
   宿主侧通过 `UpdateContext` 在稍后回调 AS3 函数。改的是 Ruffle 的 external 边界，不碰 AVM2 内核。
2. **AS3 垫片**（更快，但会改动游戏代码）：写一个极小的 SWF/ABC 前置层，用纯字符串参数把 29 个功能
   重新表达成扁平 EI 调用，内部用 `loadBytes` 载入原 SWF。属于"不完全一致"的改动，但实现成本最低（≈3–5 人日）。

---

## 5. 宿主功能清单（29 个 + 生命周期）

| 接口 | 数量 | 需要落地的跨平台后端 |
|---|---|---|
| `GameServicesInterface` | 9（登录、排行榜读写、成就读写/重置、对局、在线结果回传） | Apple Game Center / Google Play Games / Steamworks |
| `NativeUtils` | 10（语言、大屏、低内存、分享、平台 ID、Rovio 新闻、评分、splash、NTP 时间） | 各平台原生 API + 自建/公共 NTP（`rsntp`） |
| `FlurryInterface` | 6（事件、计时事件、错误、年龄、性别、用户 ID） | 可选：接到自选分析后端；不接则打日志 |
| `IAPInterface` | 3（商品信息、购买、恢复） | StoreKit 2 / Play Billing / Steam 微交易 |
| 生命周期回调 | `_root.onPaused` / `_root.onResumed`、splash 移除、方向、60fps | winit 事件 + 状态机 |

注意：**平台功能可以降级**。`--dummy-external-interface` 已经证明：这些调用全部返回空，游戏依然能起、
能进菜单。所以"先跑起来、再补平台功能"是可行的增量路径。

---

## 6. "精确"到底能做到什么程度

必须先把"精确"分档，否则无法验收：

| 档位 | 定义 | 可达性 | 手段 |
|---|---|---|---|
| **L1 逻辑精确** | 关卡判定、目标、存档、解锁顺序与原版一致 | ✅ **天然成立** | 玩法逻辑就是原版 2.48 MB ABC 本身，没被翻译；用脚本化通关 + 存档 diff 验证 |
| **L2 视觉近似** | 同分辨率下逐屏人眼不可区分；SSIM/PSNR 达标 | ✅ 高概率可达 | 修文本/字体、滤镜、混合、AA；逐屏 golden 比对 |
| **L3 时序一致** | 动画/物理在相同时间轴上前进 | 🟡 需要调 | Ruffle 是固定步长模型；原版是 `Advance(dt)` 可变步长。AS3 侧用 `getTimer()` 差值，基本自洽，但要验证 |
| **L4 像素精确** | 逐像素 bit-exact | ❌ 不现实 | 需要重写 Scaleform 矢量光栅化器与字体纹理管线 |

**只剩 L2/L3 的差距，来源很具体：**
- Scaleform 自家矢量抗锯齿/渐变/滤镜（本作实际用到 `BlurFilter/GlowFilter/DropShadowFilter/BevelFilter/ColorMatrixFilter`，各 3–4 处）；
- 文本：Scaleform 的 `FontTextureInfo` 纹理字体路径 vs Ruffle 的字体渲染（截图看主菜单字体已经很像，需要逐屏核）；
- `scaleform.gfx.*` 扩展类（本作 ABC 里只有 2 处引用，多半包在 try/catch 里）；
- 混合模式 / 遮罩 / 位图缩放采样。

### 6.1 一个绕不过去的验收难题

**原版在现代设备上跑不起来**：这个二进制是 **armv7 单架构 + 最低 iOS 7**，而 iOS 11 起彻底移除 32 位支持。
也就是说，你无法在今天的 iPhone 上开一个"参考实现"来逐帧 diff。

可选办法（按成本排序）：
1. 找一台 iOS ≤10 的旧设备（含越狱）录屏作为参考；
2. 用旧版 Xcode（6–8）+ 旧 iOS 模拟器（需要旧 macOS 或虚拟机）；
3. 接受"逻辑正确 + 视觉抽查"作为验收标准（推荐，性价比最高）。

这也反过来解释了这个项目的价值：**复刻宿主实际上是让这款游戏在 64 位/现代平台上"复活"**，
而不只是移植。原版已经没有任何可运行的现代环境了。

---

## 7. 里程碑与工作量

| 阶段 | 内容 | 验收 | 估算 |
|---|---|---|---|
| **M0 Spike** | 嵌入 Ruffle + 解密资产管线 + 启动到主菜单 | 截图与 `evidence/ruffle_menu.png` 一致 | **3–5 人日**（本次实测已完成大半） |
| **M1 宿主骨架** | ExternalInterface 全量占位 + 日志 + 输入映射 + 存档 + `onPaused/onResumed` + splash | 能进任意关卡、能存档读档、暂停恢复正确 | **2–3 人周** |
| **M2 平台服务** | IAP/成就/排行榜/分享/统计/NTP/评分，抽 `PlatformServices` trait | 各平台真机跑通购买与成就 | **4–6 人周** |
| **M3 保真度长尾** | 文本/字体、滤镜、混合、AA、时间步进、音频 | 逐屏 SSIM 达标 + 通关无逻辑偏差 | **6–10 人周** |
| **M4 多平台与打包** | Win/Linux/macOS + Android/iOS/WASM、手柄、分辨率适配、签名分发 | 各平台冒烟 + 性能 60fps | **4–8 人周** |
| **M5 加固** | 崩溃处理、遥测、版本迁移、Ruffle 升级策略 | 稳定运行 24h | 持续 |

合计：**约 17–29 人周（4–7 人月）**，加 30% 缓冲 ≈ **5–9 人月**。
如果只做"桌面能玩"，**M0+M1 ≈ 3–4 人周**。

---

## 8. 风险登记表

| 风险 | 概率 | 影响 | 缓解 |
|---|---|---|---|
| Ruffle 的 AVM2 在某个关卡踩到未实现特性 | 中 | 卡关 | 本次实测 0 报错是强信号；建立"逐关 smoke test"尽早暴露 |
| `ExternalInterface` 对象/回调无法桥接（已实测） | **高** | 内购/成就回调不可用 | 打 Ruffle 补丁（1–3 人周）或 AS3 垫片（3–5 人日） |
| 文本/字体渲染差异 | 中高 | 视觉不精确 | 优先用内嵌字体；逐屏比对；必要时自带 FreeType 提示参数对齐 |
| 滤镜/混合/遮罩差异 | 中 | 局部画面不一致 | 用 Ruffle 现有实现 + 针对性修 |
| 时间步进差异导致动画/物理轻微不同 | 中 | 手感/判定边缘 | 用固定 dt 注入 + 与录像比对 |
| 音频后端差异（无 FMOD DSP） | 低 | 音色细微不同 | 内容本身是明文 MP3，差异主要在混音/压缩器 |
| `ruffle_core` 不在 crates.io，API 不稳定 | 中 | 升级即破坏 | 锁 tag + 维护 fork；把宿主与 Ruffle 的接缝收窄到 3 个 trait |
| 多平台系统 SDK 需要 FFI（StoreKit/GK/JNI） | 低 | 引入非纯 Rust 代码 | `objc2` / `jni` crate，仍是 Rust 生态 |
| **法务**：游戏素材/代码是 Rovio 版权 | **高** | 无法公开分发 | 仅限自持副本的研究/保存；公开分发需授权 |

---

## 9. 结论与建议

1. **技术上完全可行**，而且比我预期的更接近：现成 Ruffle 已经把菜单跑出来了，缺口被量化成 9 个外部调用名。
2. **不要从零写 Flash 运行时**。正确姿势是"Ruffle 当内核，Rust 写宿主"，这样才配得上"纯 Rust/wgpu"且能在合理工期内完成。
3. **先做 M0/M1 的 2 周 Spike**：把 4 个接口按 `ExternalInterfaceProvider` 接上（先全打日志/返回空），
   确认能进关卡、能存档、能暂停恢复。这一步就能给出 90% 的可行性答案。
4. **"精确"建议这样定义并写进验收**：L1 逻辑精确（必须）、L2 视觉近似（SSIM + 人工逐屏）、L3 时序抽查；
   **明确放弃 L4 像素级精确**，性价比极低。
5. **法务先想清楚**：这套东西公开分发的风险远大于技术风险。

---

## 附录 A：复现本评估的实验步骤

```bash
# 1) 解密内容（AES-256-CBC + 去 PKCS7）
python3 tools/decrypt_assets.py            # -> decrypted/

# 2) 取 Ruffle 0.7.1（macOS universal）
curl -L -o ruffle.tgz https://github.com/ruffle-rs/ruffle/releases/download/v0.7.1/ruffle-0.7.1-macos-universal.tar.gz
tar xzf ruffle.tgz

# 3) 预置 OpenH264（可选，仅 H.264 视频用；本作 ABC 里没有 flash.media.Video，
#    只有极少量 NetStream 符号，因此在 preferences.toml 里 enable_openh264 = false 即可）
#    否则首次运行会尝试联网下载并弹许可证对话框
# 4) 无头运行（注意：Ruffle.app 带沙盒，SWF 需放在其容器目录内）
Ruffle.app/Contents/MacOS/ruffle --no-gui --dummy-external-interface \
    --graphics metal --storage memory "/path/in/container/TinyThief.swf"
```

- 证据截图：`evidence/ruffle_menu.png`
- 证据日志：`evidence/ruffle_run.log`
- 资产管线：`tools/decrypt_assets.py`
- 上一轮架构逆向：`TinyThief_架构分析.md`
- IDB（已重命名 28 个函数）：`TinyThief.app/TinyThief.i64`

## 附录 B：为什么说"游戏逻辑不用碰"

主 SWF 的 `DoABC` 是 2.48 MB 的 AVM2 字节码，其中包含 `org.j2dm.*` 玩法框架与全部关卡状态机。
在原版里它跑在 Scaleform 的 AVM2 上；在 Ruffle 里它跑在 Ruffle 的 AVM2 上。
**只要虚拟机语义一致，玩法行为就一致** —— 这就是为什么这条路线的"逻辑保真"是天然的，
而工作量集中在宿主能力与视觉长尾。
