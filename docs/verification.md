# 验收记录 — M2（原生接口 / 音频 / 存档 / 输入）

日期：2026-11-15
环境：macOS（Apple M2 Max, Metal）、Rust 1.100、Ruffle v0.7.1（git tag）
命令：`./scripts/verify.sh ../TinyThief.app`

## 1. 构建与单测

```
cargo build -p tt-host        # 通过，0 warning
cargo test -p tt-assets -p tt-platform
    tt-assets:  5 passed   (含 87 个 SWF 全量解密 + PKCS7 + 路径穿越)
    tt-platform: 1 passed
```

## 2. 启动链路（宿主日志实测）

```
TinyThief.swf
 → swf/logosState.swf
 → config/sounds/{Fx,Animations}_config.xml
 → swf/commonGamePlay.swf
 → swf/levelTool.swf
 → swf/splashScreen.swf → swf/windowGift.swf → swf/menu.swf
 → swf/chest.swf
 → sounds/menu/*.mp3
```

ExternalInterface 实测调用（去重）：

```
FlurryInterface.init / logEvent(×3, 带 Customer_ID/Session_ID 元数据)
GameServicesInterface.init / loginUser
IAPInterface.init / retrieveProductsInfo(7 个商品 ID)
NativeUtils.getPlatformID / getDeviceLanguage / getNTPTime
```

## 3. 主菜单

- 无头截图：`build/verify/menu.png`（600 帧 = 20s 游戏时间）
- 对照：`evidence/host_m2_menu.png`
- 说明：捕捉前会先 `preload()` 完成资源加载，因此不同机器/多次运行结果可复现
  （`scripts/verify.sh` 里有两次相同运行的比对断言）

## 4. 输入 → 关卡流程（三连点击）

脚本化点击序列：菜单 Play（640 帧）→ 开场漫画确认（1500 帧）→ 关卡卡片 Play（2200 帧）。

日志显示游戏随即加载：

```
fetch url=swf/levelMarketMeatNew.swf
fetch url=swf/levelTutorialGeneric.swf
```

逐步截图（均已对照验证）：

| 界面 | 文件 |
|---|---|
| 主菜单 | `build/verify/menu.png` |
| 关卡开场漫画 | `evidence/host_m2_episode_intro.png` |
| 关卡任务卡片（Medieval Burger） | `evidence/host_m2_level_card.png` |
| **关卡内实机画面** | `evidence/host_m2_gameplay.png` |

关卡内再点击地面（`--click 700,600@2400`）截图 `build/verify/gameplay_walk.png`，
与未点击时不同 → 玩法输入生效（"Tap on the floor to walk!" 教程已出现）。

## 5. 存档

首次运行后产生：

```
saves/localhost/<abs-swf-path>/analytics_data.sol
saves/localhost/<abs-swf-path>/tinyThiefDataNew.sol
```

二次运行时 Flurry 事件的 `Session_ID` 由 1 变为 2 → 说明 `.sol` 被正确读回。

## 6. 音频

- `cpal audio backend ready`，MP3 由 Ruffle 内建 symphonia 解码；
- 未开启 `mp3` feature 时日志出现 `Invalid sound: Unhandled compression Mp3`，
  游戏卡在标题画面 —— 已通过 `ruffle_core features = ["mp3","audio"]` 修复。

## 7. 本轮修复的问题

| 问题 | 根因 | 修复 |
|---|---|---|
| 主菜单后无输入响应 | 我在 capture 前把点击注入在菜单出现之前（第 240 帧是标题画面） | 改为第 640 帧点击 |
| 音频开启后卡在标题 | `ruffle_core` 未启用 `mp3` feature | 打开 `mp3`/`audio` |
| 商品 ID 列表为空 | 桥接只取顶层 `String`，AS3 传的是 `Value::List` | 递归展平 List |
| Flurry 事件丢参数 | `Analytics::log_event` 只接名字 | 增加 `params` 元数据 |

## 8. 生命周期暂停

原版在 `applicationWillResignActive:` 里调用 AS3 的 `_root.onPaused`。Ruffle 没有
宿主直接调用 AS3 属性的公开 API（`call_internal_interface` 只对应 `addCallback` 注册表，
本作实测注册 0 次），因此宿主采用等价实现：**暂停时不再 `tick()`**。

验证（同一进程内，可复现）：

```
--frames 900 --pause-after 700 --pause-frames 150 \
  --snapshot 720:pause_a.png --snapshot 840:pause_b.png
```

暂停区间内两帧完全一致；去掉暂停参数后两帧不同 → 暂停确实冻结了舞台。
窗口模式下由 `WindowEvent::Focused(false)` / `Occluded(true)` / `suspended()` 触发，
恢复时重置 `last_tick` 避免一次性大 dt。

## 9. 免费内购（购买时免单）

游戏内的内购商品一律按 `Free` 报价并在点击 Buy 后立即完成交易：

- `IAPInterface.retrieveProductsInfo` → 宿主回 `IAPInterfaceOnProductInfoReceived(ids, 0.0…, "Free"…)`；
- `IAPInterface.requestPayment(id)` → 宿主回 `IAPInterfaceOnPaymentCompleted(id, true)`；
- `restoreCompletedPurchases` → `IAPInterfaceOnRestorationCompleted(true)`。

实测（脚本化点击：进入章节地图 → 点 "Unlock Now!" → 点 Buy）：

```
INFO  tt_host::bridge: iap: granting purchase for free id=com.roviostars.tinythief.unlock_episode_royal
DEBUG tt_host::bridge: queue root call name=IAPInterfaceOnPaymentCompleted args=[String("...unlock_episode_royal"), Bool(true)]
DEBUG tt_host::host: invoking root AS3 method name="IAPInterfaceOnPaymentCompleted"
```

结果：解锁弹窗显示 `Free` + `Buy`（`evidence/host_free_iap_popup.png`），
购买后章节解锁、地图上出现星数/宝箱计数（`evidence/host_free_iap_unlocked.png`），
存档落盘为：

```
history4                     [true, false, false, false, false]
```

> 这一步依赖 vendor 的 Ruffle 补丁（`Player::call_root_method`），
> 原因与重放方法见 [`ruffle-patch.md`](ruffle-patch.md)。

## 10. 平台服务：空实现

按设计，宿主只实现接口、不实现后端：

- `Platform::null()` —— 4 个接口 29 个函数全部接线，调用会记录日志但**没有任何副作用**；
- 不创建 `state/` 目录、不写分析/内购/排行榜文件；
- `GameServices.login` 返回 `Failed`、`Iap.*` 只打日志、`NativeUtils.getNTPTime` 回退系统时钟、
  `Flurry.*` 走 `debug!`。

实测（日志）：

```
game-services.login (null backend)  ->  Failed { code: 1, .. }
iap.retrieve_products([7 ids]) (null backend)
analytics.log_event("Session Started") params=[..]   # 仅日志
```

游戏照常启动并进入关卡；验收脚本断言 `state/` 不存在。

## 11. 存档路径规范化

Ruffle 用 SWF URL 生成存档键（`localhost/Volumes/…/TinyThief.swf/save`）。
宿主加了 `NormalizedStorage` 包装，只保留最后两段，落盘变成：

```
saves/TinyThief.swf/tinyThiefDataNew.sol
saves/TinyThief.swf/analytics_data.sol
saves/TinyThief.swf/Sound.sol
```

## 12. 尚未完成（下一轮）

1. 平台服务保持空实现（按要求）；真实后端接入留待后续。
2. `onPaused/onResumed` 已通过 vendored 补丁真正调用到 AS3，同时保留冻结行为。
3. 触摸手势：Ruffle 的 `PlayerEvent` 没有 Touch 变体（只有鼠标/键盘/手柄），
   多点手势需要上游支持；单点点击/拖拽已可用。
4. 内购/成就等依赖平台后端的流程在空实现下无法验证（接口接线由日志覆盖）。
