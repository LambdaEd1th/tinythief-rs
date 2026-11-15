# Tiny Thief 1.2.2 (iOS) — 二进制架构逆向分析

> 目标：`TinyThief.app/TinyThief`（Mach-O，armv7）
> 工具：IDA Pro 9.4 + ida-mcp（Hex-Rays 反编译、vtable/ObjC 元数据、字符串交叉引用）
> 分析产物：IDB 已保存于 `TinyThief.app/TinyThief.i64`（含 28 处函数重命名与关键注释）

---

## 0. 一句话结论

**这不是一个原生 C++ 游戏引擎，也不是 Adobe AIR 容器。**

它是一个 **Autodesk Scaleform GFx 4.x（FxPlayer / Platform SDK）宿主程序**：
游戏逻辑（玩法、关卡、UI、过场）是跑在 Scaleform 内嵌 **AVM2 / ActionScript 3 虚拟机**里的 SWF 字节码（基于 `org.j2dm.*` 这套 2D 玩法框架，即 "J2DM"），
原生二进制只负责：**平台宿主 + OpenGL ES 渲染 + 触摸/手势输入 + FMOD 4 音频 + 计费/社交/统计等系统桥接**。
所有内容资源（`.swf` / `.xml` / `.mp3`）在磁盘上都是 **AES-256-CBC 整文件加密**，由原生 `FileOpener` 在加载时透明解密。

判定依据（逐条都能在二进制里对上）：

| 现象 | 证据 |
|---|---|
| Scaleform 宿主 | `Scaleform::Platform::AppBase` / `AppImpl` / `Device` / `Scaleform::Render::GL::HAL`（从 ObjC 方法 type-encoding 里漏出的 C++ 类型名）、`Scaleform ` / `Scaleform Renderer` / `scaleform.gfx.*` 字符串 |
| 内嵌 AS3 虚拟机 | `avmplus`、`VerifyError`、`ExternalInterface`、`SharedObject`、`scaleform.gfx.MouseEventEx` 等，以及 SWF 里的 `DoABC` 标签 |
| 游戏逻辑在 AS3 | 主 SWF 的 `DoABC` 达 2.48 MB，包名 `org.j2dm.*`、`entities.*`、`states.levels.*`、`characters.*`、`stageCamera.*`…… |
| 原生只做胶水 | 原生里出现的游戏相关逻辑仅是 4 个 `*.init` 分发和 29 个 `_xxx` 系统函数 |
| FMOD 4 | `FMOD::SystemI` / `FMOD::CodecVorbis` 等 mangled C++ 导出 + 构建路径 `.../fmod4/iphone/src/fmod.cpp` |
| 内容加密 | 见 §7，已实测解密成功 |

---

## 1. 二进制基本面

```
TinyThief:
  Mach-O MH_MAGIC / ARM V7 / EXECUTE / PIE / NOUNDEFS DYLDLINK TWOLEVEL
  单架构（只有 armv7，无 arm64），32 位
  文件大小 6,150,768 B
```

| 项 | 值 |
|---|---|
| Bundle ID | `com.roviostars.tinythief` |
| 版本 | 1.2.2 (CFBundleVersion 1.2.2) |
| 最低系统 | iOS 7.0 |
| 构建环境 | Xcode 6.0 (6A317)、iOS SDK 8.0 (12A365) |
| `__TEXT` | 5,537,792 B（其中 `__text` 从 `0x9780` 到 `0x4B6DC4`，约 **4.68 MB 代码**） |
| 函数数 | **19,420** 个（IDA 自动识别） |
| 符号 | `LC_SYMTAB` 仅 719 项、导出 84 个 → 本地符号基本被 strip |
| ObjC 类 | 仅 87 个（薄薄一层壳），大量是 Flurry SDK 内部类 |
| 链接框架 | UIKit / Foundation / CoreGraphics / QuartzCore / **OpenGLES** / AudioToolbox / CoreAudio / CFNetwork / SystemConfiguration / Security / **StoreKit** / **GameKit** / AdSupport / Social / Twitter / MessageUI / libobjc / libc++ / libSystem |

因为符号被 strip，IDA 里大部分是 `sub_XXXX`；能直接读的结构信息主要来自三条线：
1. ObjC runtime 元数据（类/方法/type-encoding）；
2. 未 strip 的 C++ 导出（FMOD 全量 mangled 名，说明 FMOD 是静态库静态链接进来的）；
3. vtable（C++ 虚表在 `__const` 区，可完整枚举，见 §4）。

---

## 2. 分层架构总览

```
┌──────────────────────────────────────────────────────────────────────────┐
│ iOS 应用外壳 (Objective-C)                                                │
│   FxPlayerApp (UIApplicationDelegate)  →  ScaleformAppVC (UIViewController)│
│   →  FxPlayerView (UIView, CAEAGLLayer + EAGLContext GLES2)               │
│   附带: TinyReceipt / Appirater / NetworkClock(ios-ntp) / Flurry          │
├──────────────────────────────────────────────────────────────────────────┤
│ Scaleform GFx 4.x 运行时 (C++)                                            │
│   Scaleform::Platform::AppBase/AppImpl/Device   ← Scaleform Platform SDK   │
│   GFx::Loader / MovieDef / Movie                 ← SWF 加载、播放          │
│   AVM2 (avmplus)                                 ← ActionScript 3 虚拟机   │
│   Scaleform::Render::GL::HAL                     ← OpenGL ES 2.0 渲染      │
│   Scaleform Sound (FMOD 后端) / AS3 Socket       ← 音频 / 网络             │
├──────────────────────────────────────────────────────────────────────────┤
│ 游戏宿主子类  TinyThiefApp : AppBase  (vtable @ 0x54D938, 49 个虚函数槽)  │
│   Initialize / Update / OnAppPause / OnAppResume / 事件路由 / 外部接口分发 │
├──────────────────────────────────────────────────────────────────────────┤
│ 原生扩展桥 (给 AS3 用的系统能力)                                          │
│   GameServices(9)  NativeUtils(10)  Flurry(6)  IAP(3)                     │
├──────────────────────────────────────────────────────────────────────────┤
│ AS3 游戏层 (跑在 AVM2 里)                                                 │
│   TinyThief.swf(主逻辑) + 87 个关卡/系统 SWF                              │
│   org.j2dm.* 玩法框架 + entities/states/characters/stageCamera/...        │
├──────────────────────────────────────────────────────────────────────────┤
│ 内容资源 (SWF/XML 磁盘上 AES-256-CBC 加密，MP3 为明文)                     │
│   swf/ 87 个 · config/camera 119 个 XML · config/terrain 66 个 XML        │
│   sounds/ 929 个 mp3(明文) · config/sounds/*.xml                           │
└──────────────────────────────────────────────────────────────────────────┘
```

---

## 3. 启动链路与帧循环

### 3.1 启动

```
main  @0x188C0
  ├─ signal(SIGPIPE, SIG_IGN)
  ├─ NSAutoreleasePool
  └─ UIApplicationMain(...)            ← Info.plist: NSMainNibFile = MainWindow
        └─ FxPlayerApp::applicationDidFinishLaunching:  @0x1810C
             ├─ +[Flurry startSession:@"JMJ7DZPH85QNHHZG8X2R"]
             ├─ [[ScaleformAppVC alloc] init] / initOrientations
             ├─ window.rootViewController = ScaleformAppVC
             ├─ self.view = vc.view              ← 触发 ScaleformAppVC::loadView @0x13E70
             │     ├─ FxPlayerView initWithFrame:          @0x16264
             │     │     ├─ CAEAGLLayer: RGBA8 / retained=NO / contentsScale = screen.scale
             │     │     ├─ EAGLContext initWithAPI:2      ← OpenGL ES 2.0
             │     │     └─ 注册手势: pinch / rotate / swipe×4, 多点触控开启
             │     ├─ pApp = view->ivar[14]  (Scaleform AppBase*)
             │     ├─ [pApp->vtable[2]](...)               ← App::Initialize @0xD5E4
             │     └─ setAnimationInterval: 1/60 (0x3F911111... = 0.01666s)
             ├─ 显示 Rovio Stars 启动图 (UIImageView, rovioStarsSplash 资源)
             ├─ [pApp->vtable[34]](pApp)                   ← 启动完成后回调
             ├─ startAnimation                             ← NSTimer 60Hz
             ├─ +[Appirater setAppId:@"615252129"] ... appLaunched:
             ├─ +[NetworkClock sharedNetworkClock] networkTime   ← SNTP 校时
             ├─ [SKPaymentQueue defaultQueue] addTransactionObserver:vc
             └─ [TinyReceipt purchasedAppVersion:]  @0x18614    ← 收据校验回调
```

`Info.plist` 里 `NSMainNibFile=MainWindow`，即 AppDelegate / 主窗口来自 nib，`FxPlayerApp` 是 nib 里的 delegate 类。

### 3.2 帧循环

```
NSTimer(1/60) → -[FxPlayerView updateFrame] @0x16EDC
      → pApp->vtable[3](pApp, 1)            ← TinyThiefApp::Update @0xE870
            ├─ 取设备时间 (ms)，算 dt（或按固定 1000/fps）
            ├─ movie->vtable[+100](movie, dt, 0, 1)   ← GFx::Movie::Advance(dt)
            ├─ 若 device 请求退出 → 收尾
            └─ renderer begin frame / end frame（Scaleform::Render::GL::HAL → EGL 呈现）
```

帧率常量：`animationInterval = 0x3F91111111111111 ≈ 0.016667`（60 FPS）。

### 3.3 前台/后台

App 生命周期的暂停/恢复被转发到 Scaleform 的 App 事件，并**直接回调 AS3**：

```
-[FxPlayerApp applicationWillResignActive:] → vtable[33] → TinyThiefApp::OnAppPause @0xF148
        → movie->Call("_root.onPaused", "")       // AS3 侧函数
        → movie->vtable[+124](1)                  // 暂停播放
-[FxPlayerApp applicationDidBecomeActive:]  → vtable[34] → OnAppResume @0xF198
        → movie->Call("_root.onResumed", "")
```

`_root.onPaused` / `_root.onResumed` 是 AS3 与原生约定好的钩子，这一点很关键：**原生只调这两个入口，暂停相关的业务逻辑全在 AS3**。

---

## 4. Scaleform 运行时细节（原生层）

### 4.1 类型证据

ObjC 方法的 type-encoding 里直接泄漏了 C++ 模板类型名（因为 `FxPlayerView` 的 ivar 是 C++ 指针）：

```
^{AppBase=^^?{SystemCursorManager=^^?iBBBB^{SystemCursorManagerImpl}}
        {Ptr<Scaleform::Platform::GestureManager>=^{GestureManager}}
        ^{Device}^{AppImplBase}^{AppImpl}B}
^{AppImpl=^^?^^?^{AppBase}{ViewConfig=Iiii{Size<unsigned int>=II}II{Point<int>=ii}...}}
^{DeviceImpl=^{AppImpl}{Ptr<Scaleform::Render::GL::HAL>=^{HAL}}iiiIIIB}
```

这说明用的是 **Scaleform Platform SDK 的 App/Device/View 抽象层 + GL 后端**（GFx 4.x 形态），而不是老式 `GFxPlayer` 单文件示例。

### 4.2 游戏宿主类（TinyThiefApp）

类在 `sub_C5A0`（构造）里建立，vtable 位于 `0x54D938`，共 **49 个有效虚函数槽**。其中与架构相关的：

| slot | 地址 | 语义（已重命名） | 说明 |
|---|---|---|---|
| 2 | `0xD5E4` | `TinyThiefApp::Initialize` | 建立 renderer HAL、事件路由、GL device、loader，装载 `TinyThief.swf`，接好 movie 回调 |
| 3 | `0xE870` | `TinyThiefApp::Update` | 每帧：推进 movie + 渲染 |
| 6 | `0xCBD0` | `LoadStartupMovie` | 启动加载的 SWF 名（含 "GFX or SWF file to load at startup" 提示串） |
| 33 | `0xF148` | `OnAppPause` | 回调 `_root.onPaused` |
| 34 | `0xF198` | `OnAppResume` | 回调 `_root.onResumed` |
| 47 | `0xCA38` | `HandleEvent` | Scaleform App 事件分发 |
| 48 | `0xD178` | `ExternalInterfaceHandler` | AS3→原生 外部接口调用入口（见 §6） |
| 53 | `0xD5D0` | `FSCommandHandler` | `fscommand` 通道入口 |

`Initialize`（`0xD5E4`，3.5 KB，Hex-Rays 470 行）里的关键动作：
- 通过 `dword_5A0F94`（全局分配器，Scaleform `New/Delete` 包装）分配一系列 handler 对象，用 `loader->vtable[12](loader, N, obj)` 按事件类型注册；
- 装配 GL 渲染器（`sub_10BA8` / `sub_10BAC` / `sub_109D4` / `sub_10AA0`，即 HAL 的纹理缓存/混合/视口参数）；
- `sub_FDC0(app, args, "FxPlayer", 3)` —— 保留了 FxPlayer 示例的初始化参数解析；
- `sub_6A138` / `sub_6A18C` → GFx Loader 的 `LoadMovie`，产出 **MovieDef (app+124)** 和 **Movie (app+128)**；
- movie 创建后：`movie->vtable[+52]` 设置 viewport、`Advance(0,0,1)`、更名、注册 `+144`/`+176` 回调、`sub_D074`（本文件里就是注册原生扩展的入口链）；
- 设置 `app+520 = "TinyThief.swf"`。

### 4.3 渲染

- `EAGLContext initWithAPI:2` → **OpenGL ES 2.0**；
- `CAEAGLLayer`，`kEAGLDrawablePropertyColorFormat = kEAGLColorFormatRGBA8`，`RetainedBacking = NO`；
- `contentsScale = [UIScreen mainScreen].scale`（适配 Retina）；
- 着色器字符串在二进制里可见（`vec4 base = texture2D(srctex, tc%d);` 等），是 Scaleform 内置的 GLSL 组合器；
- 游戏本身是 2D：实体是 `J2DM_SpriteAnimation` / `J2DM_ClipAnimation`，相机配置是正交的 `zoom/center/bounding_area`。

### 4.4 输入

`-[FxPlayerView touchesBegan/Moved/Ended:]` 转成 Scaleform 输入事件；另有 `UIPinchGestureRecognizer`、`UIRotationGestureRecognizer`、4 个方向的 `UISwipeGestureRecognizer`、`UIPanGestureRecognizer`（`handlePan/handleSwipe/handlePinch/handleRotate`），映射到 `Scaleform::Platform::GestureManager`；键盘输入通过隐藏 `UITextField` 转发。

### 4.5 音频

AS3 的 `flash.media.Sound` 由 Scaleform 的声音子系统实现，后端是 **FMOD 4**（Rovio 自维护 fork）：

- 导出符号：`FMOD::SystemI`、`FMOD::ChannelSoftware`、`FMOD::CodecVorbis/MPEG/WAV/FSB5`、`FMOD::OcclusionThread` …
- 线程名/标记串：`Scaleform FMOD Update`
- 构建路径：`/Library/anthill3/agent/var/jobs/job517404/fmod4/iphone/src/fmod.cpp`，并且含 **自定义源码** `../lib/sfx/foreverb/aSfxDsp.cpp`（Rovio 自己加的音效 DSP）
- 输出：`OutputCoreAudioObjC` + CoreAudio/AudioToolbox

---

## 5. 游戏逻辑层（AS3 + J2DM）

### 5.1 SWF 规模

| 文件 | 解密后 body | SWF 版本 | tag 数 | DoABC |
|---|---|---|---|---|
| `TinyThief.swf` | 3,614,256 B | 14 | 141 | 1 个，**2,486,413 B**（ABC v16/46） |
| `commonGamePlay.swf` | 1,364,559 B | 14 | 1,446 | 1 个，58,116 B |
| `levelArmory.swf` | 931,258 B | 14 | 1,051 | 1 个，15,807 B |

结论很清楚：
- **主 SWF 装的是几乎全部游戏代码**（2.48 MB ABC）；
- **关卡 SWF 主要是美术/时间轴资源**（1000+ tag，但 ABC 只有 15–60 KB）。

### 5.2 代码组织

从 ABC 常量池里提取到的命名空间与类名，可以看出 AS3 侧是**「自研 2D 玩法框架 + 数据驱动关卡对象」**的结构：

```
org.j2dm.core.J2DM_AbstractGameLoop        ← 主循环抽象
org.j2dm.display.sprite.J2DM_AbstractComponent / J2DM_SpriteAnimation / J2DM_ClipAnimation
org.j2dm.display.sprite.behaviors.J2DM_GameObject
org.j2dm.display.ui.components.J2DM_GenericButton
org.j2dm.loader.J2DM_AbstractLoader / J2DM_AbstractLoaderProgress
org.j2dm.networking (IReceiveTrama …)
org.j2dm.display.sprite.behaviors.abstractMainManager / AbstractMainManager
entities.*                                  ← 关卡实体（数据驱动）
   AbstractLevelObjectManager / AbstractTTObject / AbstractTTObjectParameters
   interactiveObjects / activableObject / hiddenObjects / genericActivateItem
   genericLadder / genericPickedItem / genericActivateItemFrom / exit
   multiActionGenericItem / AbstractInteractiveObject
characters.*                                ← hero / npc / BasicNPC
states.levels.*  (circus / queen / …)       ← 每个世界的关卡状态机
states.LevelSelectionParams
stageCamera.model.ICinematicCameraConfiguration
levelGoals.LevelGoalsController             ← 关卡目标（三颗星）
collisions.CollisionManager
worldManager / LevelWorld
DefaultLevelConfig / ILevelConfig
com.greensock.TweenLite / motionPaths.PathFollower   ← 第三方补间库
mx.*（Flex 组件/IUIComponent/样式系统…）    ← 复用 Flex 的 UI 组件基类
```

要点：
- `AbstractTTObject` / `AbstractTTObjectParameters` 说明关卡对象是**"参数对象 + 管理器"**的数据驱动模式，跟 `config/terrain/*.xml`、`config/camera/*.xml` 一一对应；
- `J2DM_` 前缀的类是一套独立于 Flash 显示列表的 2D 游戏框架（渲染/动画/行为/主循环），游戏本体就建在它上面；
- 关卡内还会用 `ILevelConfig` / `J2DM_AbstractLoader` 去异步加载 SWF 与 XML（这就是原生 FileOpener 被触发的地方）。

### 5.3 内容组织

`swf/` 共 87 个，呈现清晰的「世界 + 公共库 + 系统」三层结构：

```
系统层:  menu / splashScreen / logosState / windowGift / chest / commonGamePlay / levelWizard
世界层:  <World>Commons.swf  +  <World>1..N.swf
   ArmoryCatacumbas(9) Armory(2) ArmoryExtras
   Catacomb(3) Cemetery(2) Circus(2) Dragon(4) Siege(3)
   PirateDocks(3) PirateIsland(2) PirateShip(2) Robot(2) Temple(4)
   WitchFactory(2) WitchSchool(2) WitchTower(2) WitchTree(4)
单关:    levelBackery / levelCandyStore / levelMarket{Meat,Vegetable,Fish,Farm}
         levelNightHouse / levelOctopuss / levelPirateTavern / levelQueen
         levelTaxOffice / levelTool / levelGarden / levelTutorial2New ...
```

---

## 6. AS3 ↔ 原生 接口（架构里最"值钱"的一层）

### 6.1 机制

**AS3 → 原生**：走 Scaleform 的 `ExternalInterface` / `fscommand` 通道。AS3 侧以"方法名"字符串调用，原生侧在 `0xD18C` 里用 `strcmp` 分发（这就是外部接口 handler）：

```
"GameServicesInterface.init" → new GameServices  (vtable 0x54D918)
"NativeUtils.init"           → new NativeUtils   (vtable 0x54DCE8)
"FlurryInterface.init"       → new FlurryIface   (vtable 0x54DCC8)
"IAPInterface.init"          → new IAP           (vtable 0x54DDF8)
```

每个 `init` 调用里，原生把 C++ 对象指针包成 `GFx::Value`，再把该接口的**一组 C++ 成员函数**逐个包成 AS3 函数对象（`GFxMovie__CreateNativeFunction @0x7DB08`，实际走 movie 的 `vtable[+172]`），最后用 `GFxMovie__SetFunctionName @0x1FF0C4` 给函数起名 `_xxx` 挂到 AS3 传入的对象上。

**原生 → AS3**：`movie->Call(路径, 参数)`，例如 `_root.onPaused` / `_root.onResumed`；另有启动图移除（`rovioStarsSplash`）。

### 6.2 暴露给 AS3 的 29 个原生函数

**GameServices（Game Center）** — 注册于 `GameServices__RegisterNativeFunctions @0xC084`（10 次注册 / 9 个不同名字）

| AS3 函数 | 原生实现 |
|---|---|
| `_loginUser` / `_getLoginErrorStatus` | `-[ScaleformAppVC loginUser]` / `getLoginErrorStatus` |
| `_openLeaderboardPane` / `_setLeaderboardScore` | `GKLeaderboardViewController` |
| `_openAchievementPane` / `_setAchievementCompletion` / `_resetAchievements` | `GKAchievementViewController` / `GKAchievement` |
| `_openMatchmakingMenu` | GK matchmaking |
| `_receiveOnlineResult` | 回传在线结果给 AS3 |

> 小发现：`sub_C084` 里实际注册了 **10** 个函数，但 `_openAchievementPane` 被注册了 **两次**
> （第 4 个和第 7 个同名），所以去重后是 9 个。这是原生侧的一处冗余/笔误，
> 也说明这层接口是手工维护的扁平函数表。

**NativeUtils（设备/平台）** — `NativeUtils__RegisterNativeFunctions @0x198BC`

| AS3 函数 | 语义 |
|---|---|
| `_getDeviceLanguage` | 系统语言 |
| `_isLargeScreen` / `_isLowMemoryDevice` | 机型分档（决定 UI 缩放/贴图预算） |
| `_shareText` | 拉起分享（Mail / Message / Twitter / Social） |
| `_getPlatformID` | 平台标识 |
| `_openRovioNews` / `_closeRovioNews` | 打开 Rovio 新闻 WebView（`http://%@.rovio.com/content/embed/pauseMenu/...`） |
| `_promptUserToRateApp` | Appirater |
| `_removeSplashImage` | 移除 Rovio Stars 启动图 |
| `_getNTPTime` | SNTP 网络时间（防改表） |

**Flurry（统计）** — `Flurry__RegisterNativeFunctions @0x193A8`

`_logEvent`、`_endTimedEvent`、`_logError`、`_setAge`、`_setGender`、`_setUserId`

**IAP（内购）** — `IAP__RegisterNativeFunctions @0x1D110`

`_retrieveProductInfo`、`_requestPayment`、`_restoreCompletedPurchases`
（对应 `-[ScaleformAppVC retrieveProductInfo:]` / `requestPayment:` / `restoreCompletedPurchases`，`SKProductsRequest` + `SKPaymentQueue` + `TinyReceipt` 收据校验）

### 6.3 实测确认的调用约定

用 Ruffle（开源 Flash 播放器）加载解密后的主 SWF，可以完整看到 AS3 侧发起的原生调用：

```
Trying to call unknown ExternalInterface method: FlurryInterface.init
Trying to call unknown ExternalInterface method: GameServicesInterface.init
Trying to call unknown ExternalInterface method: IAPInterface.init
Trying to call unknown ExternalInterface method: NativeUtils.getPlatformID
Trying to call unknown ExternalInterface method: NativeUtils.getNTPTime
Trying to call unknown ExternalInterface method: NativeUtils.getDeviceLanguage
Trying to call unknown ExternalInterface method: GameServicesInterface.loginUser
Trying to call unknown ExternalInterface method: IAPInterface.retrieveProductsInfo
Trying to call unknown ExternalInterface method: FlurryInterface.logEvent
```

即 **AS3 一律用 `ExternalInterface.call("<接口>.<方法>", …)` 的形式调用**；
二进制里只有 4 个 `*.init` 字面量，其余 `<接口>.<方法>` 名是在 init 时按表拼出来的（与 §6.1 的分发/注入逻辑一致）。

### 6.4 这层意味着什么

- 原生部分**几乎没有游戏规则**，改玩法只需改 SWF；
- 但**平台能力全在原生**：想 hook 支付、成就、分享、统计、时间，只需盯这 29 个函数（地址已列，见 §10）；
- `_getNTPTime` 让 AS3 拿到"可信时间"，配合收据校验明显是**防改系统时间刷内购/限时内容**的设计。

---

## 7. 资源与加密（已完整破解）

### 7.1 加密方案

原生里有一个自定义的 Scaleform `FileOpener`：`FileOpener__OpenFile_AESDecrypt @0x11060`

```c
const char *ext = strrchr(filename, '.');
if (!ext || (strcmp(ext,".xml") && strcmp(ext,".swf") && strcmp(ext,".mp3")))
    return FileOpener::OpenFile_Plain(...);          // 其他扩展名不加密

f = fopen(filename, "rb");
size = ftell(f);
CCCryptorCreate(kCCDecrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding /*=1*/,
                "ypWnks9F4jrKjs1cQv3Tw32fi7epEvlu",   /* 32 字节 = AES-256 */
                0x20, NULL /* IV = 全 0 */, &cryptor);

while (!feof(f)) {                                   // 每 16 字节一块
    n = fread(buf16, 1, 16, f);
    CCCryptorUpdate(cryptor, buf16, n, out16, 16, &n);
    memcpy(dst + off, out16, n); off += n;
}
CCCryptorFinal(cryptor, ...);
```

**结论（已实测验证）**：

| 项 | 值 |
|---|---|
| 算法 | **AES-256-CBC**（`CCCryptorCreate` 的 options=1 是 PKCS7 标志；IV 传 NULL ⇒ 全 0 IV） |
| 密钥 | `ypWnks9F4jrKjs1cQv3Tw32fi7epEvlu`（ASCII，32 字节，位于 `0x504550`） |
| 填充 | **PKCS7**（所有资产都带；`CCCryptorFinal` 在运行时自动剥掉） |
| 范围 | 代码里只对 `.xml` / `.swf` / `.mp3` 三个扩展名生效 |
| 解密位置 | 加载时透明解密，内存里拿到的是明文 SWF/XML |

**实测的加密边界**（全量 1204 个文件逐个验证）：

| 类型 | 数量 | 状态 |
|---|---|---|
| `*.swf` | 88 | **100% 加密**（密文均为 16 字节整数倍，解密后为 `CWS\x0e`） |
| `*.xml` | 187 | **100% 加密**（解密后为 XML 文本，terrain 还带 UTF-8 BOM） |
| `*.mp3` | 929 | **全部是明文**（`FF FB` MPEG 帧同步头 / `ID3`），未加密 |

也就是说：尽管 `FileOpener` 的白名单里有 `.mp3`，本版本实际发布的音频是明文——
因为音频走的是 **FMOD 自己的文件读取**（`Scaleform FMOD Update` 线程 / FMOD File API），并不经过 GFx 的 `FileOpener`，
所以 `FileOpener` 的 `.mp3` 分支在本作里实际不会被触发。

### 7.2 验证结果

```
$ openssl aes-256-cbc -d -nopad -K <key_hex> -iv 00..00 -in TinyThief.swf -out plain.swf
plain.swf: CWS v14, 声明长度 3,614,264
$ openssl ... -in config/camera/Armory2_camera_config.xml
<?xml version="1.0"?> <level_camera_configuration version="1">
  <frame name="Top Left"><zoom>1.5</zoom><center><x>341</x><y>270</y></center> ...
$ openssl ... -in config/terrain/Armory_Catacumbas1_World_terrain.xml
<level_collisions_with_terrain><terrain_section name="instance1">...<base64_compressed_data>eNpj...
```

（`base64_compressed_data` 是 zlib 压缩的瓦片数据，用于碰撞。）

全量验证结果：**88/88 SWF、187/187 XML 均为加密**；**929/929 MP3 为明文**（见 §7.1 的边界表）。

> 踩坑记录：`.xml` 解密后**必须剥掉 PKCS7 填充**，否则根元素后面会残留 `0x01` / `0x0D0D…` 等填充字节，
> XML 解析器会报 `Error #1088: markup in the document following the root element must be well-formed`
> （这条错误是我们用 Ruffle 实跑时抓到的，见 §12 的实测）。SWF 的填充位于 zlib 流末尾，剥离后
> `CWS` 声明长度与实际 body 完全吻合；不剥也能被 zlib 忽略，但正确做法是剥掉。

批量解密脚本：`tools/decrypt_assets.py`，输出 `decrypted/`（275 个解密 + 929 个原样拷贝）。

### 7.3 资源清单

| 内容 | 数量 | 说明 |
|---|---|---|
| `TinyThief.swf` | 1 | 主程序（全部游戏代码） |
| `swf/*.swf` | 87 | 关卡 / 世界公共库 / 系统界面（menu、splash、礼物窗…） |
| `config/camera/*.xml` | 119 | 每关镜头配置（frame/zoom/center/bounding_area）+ tablet 变体 |
| `config/terrain/*.xml` | 66 | 地形碰撞 + base64+zlib 瓦片 |
| `config/sounds/*.xml` | 2 | 音效 id → mp3 路径映射（`Fx_config.xml` / `Animations_config.xml`） |
| `sounds/**` | 929 mp3 | 按世界分目录（tutorials/market/sheriff/pirates/king…），**明文未加密** |

> `TinyThief.crc` 内容为 `CheckSum=6b4f7479617261`，但**二进制里没有任何 `.crc` / `CheckSum=` 字符串引用**，判断是重打包/校验工具留下的，游戏运行时并不校验它。

---

## 8. 第三方 SDK、服务与安全

| 组件 | 证据 | 用途 |
|---|---|---|
| **Game Center** | `GKLeaderboardViewController`、`GKAchievement*`、`loginUser` | 排行榜/成就/对局 |
| **StoreKit + 收据校验** | `SKProductsRequest`、`SKPaymentQueue`、`TinyReceipt`、`AppStoreReceiptValidation.h`、`AppleIncRootCertificate.cer` | 内购 + 服务端式收据验证 |
| **Flurry** | 289 处 Flurry 字符串、`FlurrySession`、内置 **PLCrashReporter**（`FlurryPLCrash*`）、`startSession:@"JMJ7DZPH85QNHHZG8X2R"` | 统计、崩溃上报；另含 Flurry Ads/AppCloud 端点 |
| **Appirater** | `Appirater`、`setAppId:@"615252129"` | 评分提示 |
| **ios-ntp** | `GCDAsyncUdpSocket`、`NetAssociation`、`NetworkClock`、构建路径 `.../iPhone/ios-ntp/...`、包内 `ntp.hosts` | SNTP 可信时间（防改表） |
| **Social / Twitter / MessageUI** | 链接框架 + `share::`、`actionSheet:clickedButtonAtIndex:` | 分享 |
| **AdSupport** | 弱链接 | 广告标识（Flurry 用） |
| **Rovio 服务** | `http://%@.rovio.com/content/embed/pauseMenu/?d=...&p=...&a=...&v=...` | 暂停菜单内嵌网页 / Rovio 新闻 |

**安全/反调试层面**：`signal(SIGPIPE, SIG_IGN)`；收据校验走 `TinyReceipt` + PKCS#7（`AppleIncRootCertificate.cer`）；时间校验走 NTP。没有看到加壳或代码虚拟化——所以静态分析相对顺畅（本地符号被 strip 是唯一障碍）。

**关于这份拷贝**：包内 `touchg.ru` 内容为 `Cracked by touchg.ru | ipodtouch3g.ru`，另有 `ntp.hosts`，属于第三方重打包（越狱破解）版本；这也解释了“SWF 是加密的但能直接跑”的形态——内容加密是 Rovio 原始的，不是破解者加的。

---

## 9. 构建溯源

二进制里残留的绝对路径信息量很大：

```
/Users/miguel/Workspace/tt/tt.scaleform_ios/LocalApps/TinyThief/iPhone/ReceiptValidation/AppStoreReceiptValidation.h
/Users/miguel/Workspace/tt/tt.scaleform_ios/LocalApps/TinyThief/iPhone/ios-ntp/GCDAsyncUdpSocket.m
/Library/anthill3/agent/var/jobs/job517404/fmod4/iphone/../src/fmod.cpp
/Library/anthill3/agent/var/jobs/job517404/fmod4/iphone/../lib/sfx/foreverb/aSfxDsp.cpp
```

- 工程名 `tt`（Tiny Thief），目录 `tt.scaleform_ios/LocalApps/TinyThief/iPhone` —— 说明是**基于 Scaleform 官方 iOS FxPlayer 示例工程改造**，把游戏作为 `LocalApps/` 下的一个子应用；
- FMOD 是 **Rovio 自己维护的 4.x 分支**（`anthill3` 构建机、`foreverb` 音效库、`aSfxDsp.cpp` 自研 DSP）；
- 发布方：Rovio Stars（bundle id、`rovioStarsSplash`、`rovio.com` 接口）。

---

## 10. IDA 关键地址表（已写入 IDB）

| 地址 | 名称（已重命名） | 说明 |
|---|---|---|
| `0x188C0` | `main` | 入口，UIApplicationMain |
| `0x1810C` | `FxPlayerApp__applicationDidFinishLaunching` | 启动装配 + Flurry/Appirater/NTP/StoreKit |
| `0x13E70` | `ScaleformAppVC__loadView` | 建 FxPlayerView、取 AppBase、设帧率 |
| `0x16264` | `FxPlayerView__initWithFrame` | EAGL/GLES2 上下文 + 手势 |
| `0x16EDC` | `FxPlayerView__updateFrame` | 60Hz 驱动 → App::Update |
| `0xC5A0` | `TinyThiefApp__ctor` | 游戏宿主类构造（vtable 0x54D938） |
| `0xD5E4` | `TinyThiefApp__Initialize` | 渲染器/HAL/Loader/加载 TinyThief.swf |
| `0xE870` | `TinyThiefApp__Update` | 每帧推进 movie + 渲染 |
| `0xF148` / `0xF198` | `OnAppPause` / `OnAppResume` | 回调 `_root.onPaused/onResumed` |
| `0xCA38` | `HandleEvent` | App 事件路由 |
| `0xD178` / `0xD5D0` | `ExternalInterfaceHandler` / `FSCommandHandler` | AS3→原生 入口 |
| `0xD18C` | `NativeBridge__DispatchInitCall` | 4 个 `*.init` 的 strcmp 分发 |
| `0x7DB08` | `GFxMovie__CreateNativeFunction` | 把 C++ 成员函数包成 AS3 函数 |
| `0x1FF0C4` | `GFxMovie__SetFunctionName` | 命名为 `_xxx` |
| `0xC084` | `GameServices__RegisterNativeFunctions` | 9 个 Game Center 函数 |
| `0x193A8` | `Flurry__RegisterNativeFunctions` | 6 个统计函数 |
| `0x198BC` | `NativeUtils__RegisterNativeFunctions` | 10 个设备/平台函数 |
| `0x1D110` | `IAP__RegisterNativeFunctions` | 3 个内购函数 |
| `0x11060` | `FileOpener__OpenFile_AESDecrypt` | **内容解密入口（AES-256-CBC）** |
| `0x46550` | `FileOpener__OpenFile_Plain` | 非白名单扩展名直通 |
| `0x504550` | `AES256_CONTENT_KEY` | 32 字节内容密钥 |

> IDB 已保存：`TinyThief.app/TinyThief.i64`

---

## 11. 后续可做的方向

1. **AS3 层**：把 `swf/*.swf` 解密后交给 JPEXS Free Flash Decompiler（FFDec）反编译，直接看 `org.j2dm.*` 与关卡状态机；主 SWF 的 2.48 MB ABC 建议单独导出分析。
2. **关卡数据管线**：`config/terrain/*.xml` + `config/camera/*.xml` + `<World>Commons.swf` 三者对照，可还原每关的实体布局（`AbstractTTObjectParameters` 显然是反序列化这些 XML）。
3. **内购/patch 面**：`TinyReceipt` 收据校验 → `IAP__RegisterNativeFunctions` → `_requestPayment`；配合 `_getNTPTime` 的时间校验一起看。
4. **性能/机型分档**：`_isLargeScreen` / `_isLowMemoryDevice` 决定 AS3 侧的贴图与 UI 规格，可用来看机型适配策略。
5. **架构上值得注意的点**：原生层几乎不含玩法逻辑，若要"改造"这个游戏，绝大多数工作应在 AS3 层完成；原生只在这 29 个函数 + 4 个生命周期钩子上需要关注。

---

### 附：如何复现解密

```bash
KEYHEX=$(printf %s 'ypWnks9F4jrKjs1cQv3Tw32fi7epEvlu' | xxd -p -c 128)
openssl aes-256-cbc -d -nopad -K "$KEYHEX" -iv 00000000000000000000000000000000 \
        -in swf/levelArmory.swf -out levelArmory.plain.swf
```

批量脚本：`python3 tools/decrypt_assets.py`（自动区分“已加密 / 本来就明文”，
输出 `decrypted/`，可直接丢进 JPEXS FFDec 反编译）。
