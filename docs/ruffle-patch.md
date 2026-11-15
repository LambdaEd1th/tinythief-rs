# Vendored Ruffle + host→AS3 patch

## 为什么需要它

原版 iOS 宿主通过 Scaleform 的 `Movie::Call("method", args)` 主动调用 AS3：

| 调用 | 用途 |
|---|---|
| `IAPInterfaceOnProductInfoReceived(ids, prices, localised)` | 回填商品价格 |
| `IAPInterfaceOnPaymentResult(id, result)` / `IAPInterfaceOnPaymentCompleted(id, ok)` | 通知购买结果 |
| `IAPInterfaceOnRestorationCompleted(ok)` | 通知恢复购买结果 |
| `setPurchasedAppVersion(version)` | 决定付费模式 |
| `onPaused()` / `onResumed()` | 生命周期 |

Ruffle 0.7.1 **没有** 暴露任何 host→AS3 的调用面：

- `Player::call_internal_interface` 只能调用 AS3 通过 `ExternalInterface.addCallback` 注册的回调，
  本作实测注册次数为 0；
- `ruffle_core::avm2`、`ruffle_core::display_object` 两个模块对外是私有的，
  宿主无法自己构造 `Activation` 去调用 AS3。

因此把 Ruffle 源码 vendor 进来，加了一个方法。

## 补了什么

`vendor/ruffle/core/src/player.rs` 中新增：

```rust
impl Player {
    /// Invoke a method on the root AVM2 object (the document class, e.g. `Main`).
    pub fn call_root_method(
        &mut self,
        name: &str,
        args: impl IntoIterator<Item = ExternalValue>,
    ) -> ExternalValue
}
```

实现要点：
1. `context.stage.root_clip()` 拿到根影片；
2. `root.object2()` 拿根对象的 AVM2 表示（`Object::StageObject`）；
3. `Activation::from_domain(...)` 建激活，`get_public_property(name, ...)` 取方法；
4. `function.call(...)` 调用，参数用 `ExternalValue::into_avm2` 转换，返回值用
   `ExternalValue::from_avm2` 转回；
5. 根对象未就绪 / 方法不存在 / 抛异常时返回 `ExternalValue::Null` 并打日志，
   与 `Movie::Call` 的"静默 no-op"语义一致。

宿主侧 `crates/tt-host/src/bridge.rs` 用 `Outbox` 队列收集要发给 AS3 的调用，
`crates/tt-host/src/host.rs::drain_outbox` 在每个 tick 之后取出并调用
`player.call_root_method(...)`，避免重入 player。

## vendor 了哪些 crate

只保留构建 `ruffle_core` / `ruffle_render_wgpu` / `ruffle_frontend_utils` 所需的子集
（约 13 MB，不含 `tests/`、`web/`、`desktop/`、`exporter/` 与 `.git`）：

```
vendor/ruffle/{core,core/macros,core/build_playerglobal,swf,flv,wstr,
               render,render/naga-agal,render/naga-pixelbender,
               render/pixel_bender,render/wgpu,
               video,video/software,video/external,
               frontend-utils,tools/asc}
```

根 `Cargo.toml` 用 `exclude = ["vendor"]` 防止它被本仓库的 workspace 吞掉，
依赖改为：

```toml
ruffle_core = { path = "vendor/ruffle/core", default-features = false, features = [...] }
ruffle_render_wgpu = { path = "vendor/ruffle/render/wgpu" }
ruffle_frontend_utils = { path = "vendor/ruffle/frontend-utils", default-features = false, features = ["cpal","fs"] }
```

## 升级 Ruffle 时怎么重放补丁

1. 从新版本 checkout 复制上表列出的目录和根 `Cargo.toml` 到 `vendor/ruffle/`；
2. 删掉根 `Cargo.toml` 的 `default-members`，并把 `members` 收窄到上表；
3. 把 `call_root_method` 重新插到 `core/src/player.rs` 的
   `call_internal_interface` 之后（若内部 API 变动，通常只需调整
   `stage.root_clip()` / `object2()` / `Activation::from_domain` 三处）；
4. `cargo test --workspace && ./scripts/verify.sh` 重跑验收。

补丁约 70 行，不触碰 AVM2 内核、渲染与网络，升级成本可控。
