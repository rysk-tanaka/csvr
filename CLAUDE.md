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

単一ファイル構成（`src/main.rs`）。コードは3層に分かれる：

1. **データ層** — `CsvData` が CSV パースを担当。`csv` クレートで `std::io::Read` から読み込み、ヘッダーと行データを `Vec<String>` で保持
2. **レイアウト計算・データ変換** — `compute_column_widths`（先頭100行サンプリングで列幅算出）、`row_number_col_width`（行番号列の桁幅算出）、`filter_rows`（クエリによる行フィルタリング）、`compute_numeric_columns`（全行ベースで列ごとの数値判定）、`sort_indices`（列ソート）、`extract_column_values`（列から数値値抽出）、`downsample`（均等間引き）、`compute_histogram_bins`（ヒストグラムビン計算）。テスト可能な純粋関数
3. **UI 層** — `CsvrApp`（`Render` トレイト実装）がメインビュー。`TableRow`（`RenderOnce` / `IntoElement`）が個別行。本体は `uniform_list` による仮想スクロール

入力の流れ: `load_csv()`（CLI引数 or stdin） → `CsvData` → `CsvrApp::new(data, cx)` → GPUI ウィンドウ

### 状態変更パターン

`CsvrApp` の状態変更は専用メソッドに集約し、関連する副作用（フィルタ再計算、スクロールリセット等）の呼び忘れを防ぐ。

- `set_search_query()` — クエリ変更 + `filtered_indices` 再計算 + スクロール先頭リセット
- `toggle_search()` / `close_search()` — 検索状態の切り替え。`close_search` はクエリクリアを含む
- `toggle_sort(col)` — ソート状態サイクル（None → Asc → Desc → None）+ `recompute_filtered_indices` + スクロール先頭リセット
- `recompute_filtered_indices()` — フィルタ → ソートを一貫適用。`set_search_query` と `toggle_sort` から呼ばれる
- `toggle_chart()` — チャートパネルの表示/非表示切替（`Cmd+G`）
- `set_chart_type(ct)` — チャートタイプ変更（Bar / Line / Scatter / Histogram）
- `set_chart_col(col)` / `set_chart_x_col(col)` — チャート対象列の変更（数値列のみ）

### ソートの設計判断

- **数値列判定は全行ベース** — `compute_numeric_columns` は初期化時に全行をスキャン。フィルタ状態で比較モードが変わるのを防ぐため
- **`f64::total_cmp` を使用** — `partial_cmp` は NaN で `None` を返し全順序を満たさないため。`total_cmp` は NaN に対しても決定的な順序を保証（正の NaN は最大値側に配置）。パース失敗時は `NEG_INFINITY` にフォールバックし最小値側に配置
- **ソートキーは事前計算** — 数値モード時は `Vec<(usize, f64)>` を構築してからソート。`sort_by` 内での O(n log n) 回のパースを回避

### チャートの設計判断

- **`canvas` 要素で描画** — GPUI の `canvas` は prepaint/paint 2段階のレンダリング。データは `render()` 内で `ChartData` に事前計算し `move` クロージャに渡す
- **ダウンサンプリング** — Bar: 100点、Line/Scatter: 500点に均等間引きで制限。大量データ時のパフォーマンスを確保
- **Scatter の X/Y マッチング** — 行インデックスで X 列と Y 列の値を突合。両方の列に有効な数値がある行のみプロット
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
- データ処理・レイアウト計算の純粋関数をテスト対象とする

## 機能ロードマップ（優先順）

1. ~~ファイル指定またはパイプ入力で CSV を読み込み、テーブル表示~~ ✅
2. ~~列固定ヘッダー（スクロールしてもヘッダーが残る）~~ ✅
3. ~~列幅の自動調整~~ ✅
4. ~~行番号表示~~ ✅
5. ~~ヘッダーと本体の水平スクロール同期~~ ✅
6. ~~インクリメンタル検索・フィルタ~~ ✅
7. ~~列ソート（昇順/降順）~~ ✅
8. ~~グラフプレビュー（Bar / Line / Scatter / Histogram）~~ ✅
