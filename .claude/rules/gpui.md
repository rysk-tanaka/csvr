---
paths:
  - "src/**/*.rs"
---

# GPUI API（v0.188.6）

GPUI のドキュメントは限られている。Zed のソースコード（`~/.cargo/git/checkouts/zed-*/` 以下）が最も信頼できるリファレンス。

## 主要パターン

```rust
// Render トレイト — ViewContext ではなく Window + Context
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement { ... }
}

// RenderOnce — 使い捨て要素（行コンポーネント等）
#[derive(IntoElement)]
struct MyElement { ... }
impl RenderOnce for MyElement {
    fn render(self, _window: &mut Window, _cx: &mut App) -> impl IntoElement { ... }
}

// アプリ起動
Application::new().run(|cx: &mut App| { ... });

// ウィンドウ作成
cx.open_window(options, |_window, cx| cx.new(|_| MyView));

// 仮想スクロールリスト（均一行高）
uniform_list(entity, "id", item_count, |this, range, window, cx| { ... })
    .track_scroll(scroll_handle)
```

## キーボード操作・アクション

```rust
// アクション定義
actions!(csvr, [ToggleSearch, DismissSearch, ToggleChart]);

// キーバインド（コンテキスト付き — key_context に一致する要素にフォーカス時のみ発火）
cx.bind_keys([
    KeyBinding::new("cmd-f", ToggleSearch, Some("CsvrApp")),
    KeyBinding::new("escape", DismissSearch, Some("CsvrApp")),
    KeyBinding::new("cmd-g", ToggleChart, Some("CsvrApp")),
]);

// アクションハンドラ
div()
    .track_focus(&self.focus_handle(cx))
    .key_context("CsvrApp")
    .on_action(cx.listener(|this, _: &ToggleSearch, _window, cx| { ... }))
    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _window, cx| { ... }))

// Focusable トレイト — キーイベント受信に必要
impl Focusable for CsvrApp {
    fn focus_handle(&self, _: &App) -> FocusHandle { self.focus_handle.clone() }
}

// ウィンドウ作成時にフォーカスを設定
|window, cx| {
    let entity = cx.new(|cx| CsvrApp::new(data, cx));
    window.focus(&entity.read(cx).focus_handle.clone());
    entity
}
```

- `AppContext` トレイトは `gpui::prelude::*` に含まれない。`cx.new()` を使うには明示的に `use gpui::AppContext` が必要（`App` 型とは別のトレイト）
- `Focusable` トレイトは `gpui::prelude::*` に含まれない。明示的に `use gpui::Focusable` が必要
- `KeyBinding` のコンテキストは `None` にするとグローバル。将来のモーダル追加時に競合するため `Some("CsvrApp")` を推奨
- `on_key_down` はアクションにマッチしなかったキーストロークを処理する。テキスト入力は `keystroke.key_char` から取得

## 重要な型の対応

- `App` — アプリケーションコンテキスト（旧 `AppContext`）
- `Context<T>` — ビューコンテキスト（旧 `ViewContext<T>`）
- `Entity<T>` — ビューハンドル（旧 `View<T>`）
- `UniformListScrollHandle` — uniform_list のスクロール状態管理
- `ScrollHandle` — div 要素のスクロール状態管理

## スクロール実装の注意点

- `uniform_list` の `paint_scroll_listener` は `cx.stop_propagation()` を無条件に呼ぶため、スクロールイベントは親要素に伝播しない
- 横スクロールを有効にするには `with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)` を使用
- `overflow_x_scroll()` / `overflow_scroll()` は `StatefulInteractiveElement` トレイトのメソッド。`div()` で使うには先に `.id("name")` を呼ぶ必要がある
- `UniformListScrollHandle` の水平オフセットを取得する公開 API は存在しない（v0.188.6 時点）。内部フィールド `handle.0.borrow().base_handle.offset().x` への直接アクセスが必要（`h_scroll_offset()` に分離済み）

## 非公開 API の利用ルール

GPUI は公開 API が限られており、内部フィールドへの直接アクセスが必要になる場合がある。その際は以下を守ること：

1. **ヘルパーメソッドに分離** — 内部アクセスを1箇所に閉じ込め、将来の置き換えコストを最小化する
2. **HACK コメントを付与** — なぜ内部 API が必要か、公開 API が追加されたら置き換える旨を明記する
3. **GPUI バージョンアップ時に優先確認** — `HACK:` コメントの箇所を最初にチェックする
