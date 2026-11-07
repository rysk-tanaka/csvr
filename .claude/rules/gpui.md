---
paths:
  - "src/**/*.rs"
---

# GPUI API（v1.27.0）

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
gpui_platform::application()
    .with_quit_mode(QuitMode::LastWindowClosed)
    .run(|cx: &mut App| { ... });

// ウィンドウ作成
cx.open_window(options, |_window, cx| cx.new(|_| MyView));

// 仮想スクロールリスト（均一行高）
uniform_list("id", item_count, cx.processor(|this, range: Range<usize>, window, cx| { ... }))
    .track_scroll(&scroll_handle)
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
    let entity = cx.new(|cx| CsvrApp::new(data, window, cx));
    let focus = entity.read(cx).focus_handle.clone();
    window.focus(&focus, cx);
    entity
}
```

- `AppContext` トレイトは `gpui::prelude::*` に含まれない。`cx.new()` を使うには明示的に `use gpui::AppContext` が必要（`App` 型とは別のトレイト）
- `Focusable` トレイトは `gpui::prelude::*` に含まれない。明示的に `use gpui::Focusable` が必要
- `KeyBinding` のコンテキストは `None` にするとグローバル。将来のモーダル追加時に競合するため `Some("CsvrApp")` を推奨。ただし、終了のようにフォーカスに関係なく効かせる操作は `None` にする
- `on_key_down` はアクションにマッチしなかったキーストロークを処理する。`keystroke.key_char` で文字も取れるが、IME の変換中の文字や確定した文字は届かない
- テキスト入力は入力の受け口で受ける。`EntityInputHandler` を実装し、paint の中で `window.handle_input(&focus_handle, ElementInputHandler::new(bounds, entity), cx)` を毎フレーム呼ぶ（手本は Zed の `crates/gpui/examples/input.rs`）
  - 受け口を登録したら、`on_key_down` で文字を足さない。伝播を止めなかったキーの文字は GPUI が受け口に渡すので、2 回入る
  - `on_key_down` で処理したキー（Enter・Backspace など）は `cx.stop_propagation()` で止める。止めないと、web では gpui_web の見えない textarea にも改行や削除が入り、受け口にもう一度届く
  - web（gpui_web）は IME の変換中も先に `KeyDown` を配る。変換中は `on_key_down` でキーを処理せず、キーの割り当ても効かせない。割り当ては、変換中だけキーコンテキストに識別子を足し、`KeyBinding` のコンテキストを `CsvrApp && !ime_editing` のように否定で書けば 1 か所で外せる（GPUI はキーを処理する前に、変わったビューを描き直すので、コンテキストは最新になる）
- macOS の `QuitMode` の既定値は `Explicit`。最後のウィンドウを閉じても終了しないため `LastWindowClosed` を指定する

## バージョン更新時の注意点

- `gpui` と `gpui_platform` は同じタグに揃える。タグが異なると `gpui` が2バージョン混在してビルドできない
- `Pixels` の内部フィールドは非公開。`f32` への変換は `as_f32()` を使う
- Metal Toolchain がない環境では `--features gpui_platform/runtime_shaders` を付けるとビルド時のシェーダーコンパイルを省略できる

## 重要な型の対応

- `App` — アプリケーションコンテキスト（旧 `AppContext`）
- `Context<T>` — ビューコンテキスト（旧 `ViewContext<T>`）
- `Entity<T>` — ビューハンドル（旧 `View<T>`）
- `UniformListScrollHandle` — uniform_list のスクロール状態管理
- `ScrollHandle` — div 要素のスクロール状態管理

## スクロール実装の注意点

- `paint_scroll_listener` は `hitbox.should_handle_scroll` で処理の要否を判定し、`cx.stop_propagation()` は呼ばない（v1.27.0 時点）
- 横スクロールを有効にするには `with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)` を使用
- `overflow_x_scroll()` / `overflow_scroll()` は `StatefulInteractiveElement` トレイトのメソッド。`div()` で使うには先に `.id("name")` を呼ぶ必要がある
- `UniformListScrollHandle` の水平オフセットを取得する公開 API は存在しない（v1.27.0 時点）。内部フィールド `handle.0.borrow().base_handle.offset().x` への直接アクセスが必要（読み取りは `h_scroll_offset()`、書き込みは `set_h_scroll_offset()` に分離済み）

## 非公開 API の利用ルール

GPUI は公開 API が限られており、内部フィールドへの直接アクセスが必要になる場合がある。その際は以下を守ること：

1. **ヘルパーメソッドに分離** — 内部アクセスを1箇所に閉じ込め、将来の置き換えコストを最小化する
2. **HACK コメントを付与** — なぜ内部 API が必要か、公開 API が追加されたら置き換える旨を明記する
3. **リリース前に優先確認** — GPUI の minor / patch 更新は CI が通れば自動マージされるため、csvr のバージョンを上げる前に `HACK:` コメントの箇所を実機で確認する
