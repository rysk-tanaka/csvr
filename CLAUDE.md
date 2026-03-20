# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

csvr — CLI から起動する CSV ビューワー。GPUI（Zed の UI フレームワーク）で構築。macOS 専用。

## 開発コマンド

```bash
cargo run -- data.csv        # ファイル指定で実行
printf "a,b\n1,2\n" | cargo run  # パイプ入力で実行
cargo test                   # 全テスト実行
cargo test test_name         # 単一テスト実行
cargo clippy                 # lint
cargo build --release        # リリースビルド
cargo tarpaulin --skip-clean --out stdout  # カバレッジ計測
```

## ビルド前提条件

GPUI は Metal シェーダーをコンパイルするため、通常の Rust プロジェクトより多くの準備が必要。

- Xcode フルインストール（Command Line Tools だけでは不足）
- Metal Toolchain（Xcode の Settings > Components からインストール）
- `xcrun metal --version` で Metal コンパイラの動作を確認できる

詳細は [docs/setup.md](./docs/setup.md) を参照。

## アーキテクチャ

4モジュール構成。依存関係: `data`(基盤) ← `compute`, `chart` ← `app` ← `main`。循環依存なし。

```text
src/
  main.rs      -- エントリポイント（load_csv, main）
  data.rs      -- 型定義: CsvData, ChartType, SortDirection, ChartData
  compute.rs   -- 純粋計算関数 + テスト
  chart.rs     -- draw_chart + チャートカラー定数
  app.rs       -- CsvrApp, TableRow, UIカラー定数, actions!
```

1. **データ層** (`data.rs`) — `CsvData` が CSV パースを担当。`csv` クレートで `std::io::Read` から読み込み、ヘッダーと行データを `Vec<String>` で保持。`ChartType`, `SortDirection`, `ChartData` の型定義
2. **計算層** (`compute.rs`) — テスト可能な純粋関数群。列幅算出、行フィルタリング、数値列判定、ソート、チャート用データ抽出・ダウンサンプリング・ヒストグラムビン計算など
3. **チャート描画** (`chart.rs`) — `draw_chart` 関数。GPUI の `canvas` 要素の paint コールバックから呼ばれる
4. **UI 層** (`app.rs`) — `CsvrApp`（`Render` トレイト実装）がメインビュー。`TableRow`（`RenderOnce` / `IntoElement`）が個別行。本体は `uniform_list` による仮想スクロール

入力の流れ: `load_csv()`（CLI引数 or stdin） → `CsvData` → `CsvrApp::new(data, cx)` → GPUI ウィンドウ

### 状態変更パターン

`CsvrApp` の状態変更は専用メソッドに集約し、関連する副作用（フィルタ再計算、スクロールリセット等）の呼び忘れを防ぐ。

- `set_search_query()` — クエリ変更 + `filtered_indices` 再計算 + スクロール先頭リセット
- `toggle_search()` / `close_search()` — 検索状態の切り替え。`close_search` はクエリクリアを含む
- `toggle_sort(col)` — ソート状態サイクル（None → Asc → Desc → None）+ `recompute_filtered_indices` + スクロール先頭リセット
- `recompute_filtered_indices()` — フィルタ → ソートを一貫適用。`set_search_query` と `toggle_sort` から呼ばれる
- `toggle_chart()` — チャートパネルの表示/非表示切替（`Cmd+G`）
- `set_chart_type(ct)` — チャートタイプ変更（Bar / Line / Scatter / Histogram）
- `set_chart_col(col)` / `set_chart_x_col(col)` — チャート対象列の変更（数値列のみ）+ `recompute_chart_data`
- `recompute_chart_data()` — `chart_data_cache` を再計算。`toggle_chart`、`set_chart_type`、`set_chart_col`、`set_chart_x_col`、`recompute_filtered_indices` から呼ばれる

### ソートの設計判断

- **数値列判定は全行ベース** — `compute_numeric_columns` は初期化時に全行をスキャン。フィルタ状態で比較モードが変わるのを防ぐため
- **`f64::total_cmp` を使用** — `partial_cmp` は NaN で `None` を返し全順序を満たさないため。`total_cmp` は NaN に対しても決定的な順序を保証（正の NaN は最大値側に配置）。パース失敗時は `NEG_INFINITY` にフォールバックし最小値側に配置
- **ソートキーは事前計算** — 数値モード時は `Vec<(usize, f64)>` を構築してからソート。`sort_by` 内での O(n log n) 回のパースを回避

### チャートの設計判断

- **`canvas` 要素で描画** — GPUI の `canvas` は prepaint/paint 2段階のレンダリング。`ChartData` は `chart_data_cache` に保持し、状態変更時のみ `recompute_chart_data()` で再計算。`render()` ではキャッシュを clone して `move` クロージャに渡す
- **ダウンサンプリング** — Bar: 100点、Line/Scatter: 500点に均等間引きで制限。大量データ時のパフォーマンスを確保
- **Scatter の X/Y マッチング** — `extract_scatter_pairs` で1回のイテレーションで両列を同時に抽出。両方の列に有効な数値がある行のみプロット
- **ゼロ除算防止** — Bar/Line/Scatter では全値同一（range == 0）の場合 range を 1.0 にフォールバック。Histogram では全値を中央ビンに配置

## GPUI API（v0.188.6）

GPUI のドキュメントは限られている。Zed のソースコード（`~/.cargo/git/checkouts/zed-*/` 以下）が最も信頼できるリファレンス。

### 主要パターン

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

### キーボード操作・アクション

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

### 重要な型の対応

- `App` — アプリケーションコンテキスト（旧 `AppContext`）
- `Context<T>` — ビューコンテキスト（旧 `ViewContext<T>`）
- `Entity<T>` — ビューハンドル（旧 `View<T>`）
- `UniformListScrollHandle` — uniform_list のスクロール状態管理
- `ScrollHandle` — div 要素のスクロール状態管理

### スクロール実装の注意点

- `uniform_list` の `paint_scroll_listener` は `cx.stop_propagation()` を無条件に呼ぶため、スクロールイベントは親要素に伝播しない
- 横スクロールを有効にするには `with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)` を使用
- `overflow_x_scroll()` / `overflow_scroll()` は `StatefulInteractiveElement` トレイトのメソッド。`div()` で使うには先に `.id("name")` を呼ぶ必要がある
- `UniformListScrollHandle` の水平オフセットを取得する公開 API は存在しない（v0.188.6 時点）。内部フィールド `handle.0.borrow().base_handle.offset().x` への直接アクセスが必要（`h_scroll_offset()` に分離済み）

## 非公開 API の利用ルール

GPUI は公開 API が限られており、内部フィールドへの直接アクセスが必要になる場合がある。その際は以下を守ること：

1. **ヘルパーメソッドに分離** — 内部アクセスを1箇所に閉じ込め、将来の置き換えコストを最小化する
2. **HACK コメントを付与** — なぜ内部 API が必要か、公開 API が追加されたら置き換える旨を明記する
3. **GPUI バージョンアップ時に優先確認** — `HACK:` コメントの箇所を最初にチェックする

## CI

GitHub Actions は macOS ランナー（`macos-latest`）で実行。GPUI が Metal を必要とするため Linux では clippy/test/build いずれも不可。

## テスト方針

- GPUI レンダリング（`Render`/`RenderOnce` 実装）はウィンドウ環境が必要なためユニットテスト対象外
- `load_csv` は stdin/プロセス終了に依存するためユニットテスト対象外
- データ処理・レイアウト計算の純粋関数をテスト対象とする（テストは `data.rs` と `compute.rs` に配置）

### レイアウトの設計判断

- **行の最小幅にビューポート幅を使用** — `TableRow` に `min_row_width`（`window.viewport_size().width`）を渡し、`min_w(px(...))` を設定。カラム数が少ない場合でも行背景がウィンドウ端まで伸びる。カラム合計幅がビューポートを超える場合は `min_w` が無効化され、通常の横スクロールになる
- **`uniform_list` + `Unconstrained` では `flex_1` フィラーが効かない** — 各行の幅はコンテンツで決まるため、`flex_1` は伸びる余地がない。明示的な `min_w` 指定が必要

## 実装済み機能

CSV 読み込み（ファイル/パイプ）、テーブル表示（固定ヘッダー・列幅自動調整・行番号・水平スクロール同期）、インクリメンタル検索・フィルタ、列ソート（昇順/降順）、グラフプレビュー（Bar / Line / Scatter / Histogram）
