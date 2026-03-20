# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

csvr — CLI から起動する読み取り専用の CSV ビューワー。GPUI（Zed の UI フレームワーク）で構築。macOS 専用。編集・保存機能はスコープ外。

## 開発コマンド

```bash
cargo run -- data.csv        # ファイル指定で実行
printf "a,b\n1,2\n" | cargo run  # パイプ入力で実行
cargo test                   # 全テスト実行
cargo test test_name         # 単一テスト実行
cargo clippy                 # lint
cargo build --release        # リリースビルド
cargo tarpaulin --skip-clean --out stdout  # カバレッジ計測
cargo update-licenses                # THIRD_PARTY_LICENSES.html 再生成（要: cargo-about）
./scripts/screenshot.sh              # README用スクリーンショット撮影（要: Accessibility許可 / 表示環境）
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
- `recompute_filtered_indices()` — フィルタ → ソートを一貫適用 + 選択クリア。`set_search_query` と `toggle_sort` から呼ばれる
- `select_cell(filtered_idx, col)` — セル選択。`col=None` で行全体選択
- `clear_selection()` — 選択解除（Escape で呼ばれる）
- `move_selection(row_delta, col_delta)` — 矢印キーによるカーソル移動 + `ensure_visible` で自動スクロール
- `copy_selection(cx)` — 選択中のセル値（またはタブ区切り行）をクリップボードにコピー（`Cmd+C`）
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

### セル選択の設計判断

- **選択インデックスは `filtered_indices` ベース** — 表示上の位置と一致させることで矢印キー移動が直感的に動作。フィルタ/ソート変更時に `recompute_filtered_indices` で選択をクリアし不整合を防ぐ
- **行コピーはタブ区切り** — スプレッドシートへの貼り付け互換性が最も高い
- **`TableRow` に `Entity<CsvrApp>` を保持** — クリックハンドラから親の状態を更新するため。`Entity` は参照カウントされたハンドルなので clone コストは低い

## GPUI API（v0.188.6）

GPUI のドキュメントは限られている。Zed のソースコード（`~/.cargo/git/checkouts/zed-*/` 以下）が最も信頼できるリファレンス。
詳細なコード例・パターン集は `.claude/rules/gpui.md` に配置（`src/**/*.rs` 操作時に自動ロード）。以下は特に重要な注意点：

- `AppContext` トレイトと `Focusable` トレイトは `gpui::prelude::*` に含まれない — 明示的に `use` が必要
- `uniform_list` はスクロールイベントを親に伝播しない（`cx.stop_propagation()` が無条件）
- `UniformListScrollHandle` の水平オフセット取得に公開 API がない — 内部フィールドへの直接アクセスが必要（`h_scroll_offset()` に分離済み）
- `overflow_x_scroll()` は `StatefulInteractiveElement` のメソッド — `div()` では先に `.id()` が必要
- 非公開 API を使う場合: ヘルパーメソッドに分離 + `HACK` コメント付与 + バージョンアップ時に優先確認

## Rust Edition

Rust 2024 edition（`edition = "2024"`）を使用。GPUI v0.188.6 との互換性は確認済み。

## CI

GitHub Actions は macOS ランナー（`macos-latest`）で実行。GPUI が Metal を必要とするため Linux では clippy/test/build いずれも不可。

## テスト方針

- GPUI レンダリング（`Render`/`RenderOnce` 実装）はウィンドウ環境が必要なためユニットテスト対象外
- `load_csv` は stdin/プロセス終了に依存するためユニットテスト対象外
- データ処理・レイアウト計算の純粋関数をテスト対象とする（テストは `data.rs` と `compute.rs` に配置）

### レイアウトの設計判断

- **行の最小幅にビューポート幅を使用** — `TableRow` に `min_row_width: gpui::Pixels`（`window.viewport_size().width`）を渡し、`min_w(self.min_row_width)` を設定。カラム数が少ない場合でも行背景がウィンドウ端まで伸びる。カラム合計幅がビューポートを超える場合は `min_w` が無効化され、通常の横スクロールになる
- **`uniform_list` + `Unconstrained` では `flex_1` フィラーが効かない** — 各行の幅はコンテンツで決まるため、`flex_1` は伸びる余地がない。明示的な `min_w` 指定が必要
