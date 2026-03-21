---
paths:
  - "src/**/*.rs"
---

# 設計判断

## 状態変更パターン

`CsvrApp` の状態変更は専用メソッドに集約し、関連する副作用（フィルタ再計算、スクロールリセット等）の呼び忘れを防ぐ。

- `set_search_query()` — クエリ変更 + `filtered_indices` 再計算 + スクロール先頭リセット
- `toggle_search()` / `close_search()` — 検索バーの表示切替。`close_search` はバーを閉じるがクエリを維持（全コマンド共通の Escape 挙動）
- `toggle_sort(col)` — ソート状態サイクル（None → Asc → Desc → None）+ `recompute_filtered_indices` + スクロール先頭リセット
- `recompute_filtered_indices()` — `/` フィルタ → `&` 正規表現フィルタ → ソートを一貫適用 + 選択クリア。`set_search_query`、`toggle_sort`、`set_row_filter_query` から呼ばれる
- `select_cell(filtered_idx, col)` — セル選択。`col=None` で行全体選択
- `clear_selection()` — 選択解除（Escape で呼ばれる）
- `move_selection(row_delta, col_delta)` — 矢印キーによるカーソル移動 + `ensure_visible` で自動スクロール。列変更時のみ `recompute_column_stats` を呼ぶ（行のみ移動時はスキップ）
- `copy_selection(cx)` — 選択中のセル値（またはタブ区切り行）をクリップボードにコピー（`Cmd+C`）
- `toggle_chart()` — チャートパネルの表示/非表示切替（`Cmd+G`）
- `set_chart_type(ct)` — チャートタイプ変更（Bar / Line / Scatter / Histogram）
- `set_chart_col(col)` / `set_chart_x_col(col)` — チャート対象列の変更（数値列のみ）+ `recompute_chart_data`
- `recompute_chart_data()` — `chart_data_cache` を再計算。`toggle_chart`、`set_chart_type`、`set_chart_col`、`set_chart_x_col`、`recompute_filtered_indices` から呼ばれる
- `recompute_column_stats()` — `column_stats_cache` を再計算。`select_cell` から呼ばれる。`recompute_filtered_indices` ではキャッシュを直接 `None` にクリア
- `toggle_col_filter()` / `set_col_filter_query()` / `close_col_filter()` — `*` コマンドによる列フィルタ。`toggle` は `recompute_visible_columns()` で再検証（エラーフラグを正確に反映）
- `toggle_pin_input()` / `confirm_pin_input()` / `cancel_pin_input()` — `f` コマンドによる列固定。空入力で Enter → `pinned_col_count = 0`（リセット）
- `toggle_row_filter()` / `set_row_filter_query()` / `close_row_filter()` — `&` コマンドによる行の正規表現フィルタ。`toggle` は `recompute_filtered_indices()` で再検証。`col:pattern` 構文で列指定可能

## ステータスバー

- **統計は `column_stats_cache` にキャッシュ** — `chart_data_cache` と同様のパターン。`select_cell` / `recompute_filtered_indices` 時にのみ `recompute_column_stats()` で再計算。`render()` ではキャッシュを参照するだけ（ホバーやリサイズによる再描画で O(n) 計算が走るのを防ぐ）
- **median/stddev の追加で Vec 蓄積が必要に** — median はソートが必須のため単一パスでは不可能。値を `Vec<f64>` に蓄積し、sum/min/max は蓄積と同時に計算。stddev は標本標準偏差（Bessel 補正: `n-1` 除算）、2パス方式（数値安定性のため）。`count == 1` 時は `0.0`。パフォーマンス影響は `recompute_column_stats` がセル選択変更時のみ呼ばれることで限定
- **表示フォーマット** — 整数値は小数点なし、それ以外は小数4桁。`format_stat()` ヘルパーで統一。閾値は f64 仮数部の精度限界（2^53 ≈ 9.0e15）

## ソート

- **数値列判定は全行ベース** — `compute_numeric_columns` は初期化時に全行をスキャン。フィルタ状態で比較モードが変わるのを防ぐため
- **`f64::total_cmp` を使用** — `partial_cmp` は NaN で `None` を返し全順序を満たさないため。`total_cmp` は NaN に対しても決定的な順序を保証（正の NaN は最大値側に配置）。パース失敗時は `NEG_INFINITY` にフォールバックし最小値側に配置
- **ソートキーは事前計算** — 数値モード時は `Vec<(usize, f64)>` を構築してからソート。`sort_by` 内での O(n log n) 回のパースを回避

## チャート

- **`canvas` 要素で描画** — GPUI の `canvas` は prepaint/paint 2段階のレンダリング。`ChartData` は `chart_data_cache` に保持し、状態変更時のみ `recompute_chart_data()` で再計算。`render()` ではキャッシュを clone して `move` クロージャに渡す
- **ダウンサンプリング** — Bar: 100点、Line/Scatter: 500点に均等間引きで制限。大量データ時のパフォーマンスを確保
- **Scatter の X/Y マッチング** — `extract_scatter_pairs` で1回のイテレーションで両列を同時に抽出。両方の列に有効な数値がある行のみプロット
- **ゼロ除算防止** — Bar/Line/Scatter では全値同一（range == 0）の場合 range を 1.0 にフォールバック。Histogram では全値を中央ビンに配置

## セル選択

- **選択インデックスは `filtered_indices` ベース** — 表示上の位置と一致させることで矢印キー移動が直感的に動作。フィルタ/ソート変更時に `recompute_filtered_indices` で選択をクリアし不整合を防ぐ
- **行コピーはタブ区切り** — スプレッドシートへの貼り付け互換性が最も高い
- **`TableRow` に `Entity<CsvrApp>` を保持** — クリックハンドラから親の状態を更新するため。`Entity` は参照カウントされたハンドルなので clone コストは低い
- **ホバーは GPUI の `.hover()` スタイルで実装** — 状態管理不要。行 div に `.id()` を付与して `StatefulInteractiveElement` にし、`.hover(|style| style.bg(...))` で背景色を変更。選択中の行ではホバーを無効化
- **行選択時の矢印キーは非対称** — `col=None`（行全体選択）で右キー→`Some(0)` に遷移してセル選択モードに入る。左キーは `None` のまま（行選択から左に戻す意味がないため）

## 列操作

- **csvlens 互換のインタラクション** — `/`（検索）、`*`（列フィルタ）、`f`（列固定）、`&`（行フィルタ）のプレフィックスコマンド方式。全コマンドで統一された挙動: Enter でバーを閉じてフィルタ維持、Escape でバーを閉じてフィルタ維持（フィルタ解除は再度開いてクエリを削除）
- **入力モードは排他的** — `any_input_active()` で判定。優先順位: `*` > `f` > `&` > `/`。あるモード中は他のトリガーキーや矢印キーを無効化
- **`selected_cell` の col は元のカラムインデックスを保持** — 列非表示でも参照が壊れない。`move_selection` は `visible_col_indices` 内で前後に移動
- **列固定は固定列 div と非固定列 div の分離方式** — GPUI には CSS `position: sticky` がないため、行を「固定部分（行番号 + ピン留め列）」と「スクロール部分（残りの列）」の2つの div に分割。固定部分に `ml(-h_offset)` を適用してスクロールを打ち消し、背景色で非固定列の上に重ねる。ヘッダーと `TableRow` で同じパターンを使用
- **`&` フィルタは `col:pattern` 構文で列指定** — `parse_column_filter` がヘッダー名とコロンを解析。マッチしない場合は全文を全列対象パターンとして扱う
- **`*` と `&` は正規表現ベース** — `regex` クレート使用、case-insensitive。不正パターン時はフォールバック（`*` は全列表示、`&` はフィルタ無適用）

## レイアウト

- **行の最小幅にビューポート幅を使用** — `TableRow` に `min_row_width: gpui::Pixels`（`window.viewport_size().width`）を渡し、`min_w(self.min_row_width)` を設定。カラム数が少ない場合でも行背景がウィンドウ端まで伸びる。カラム合計幅がビューポートを超える場合は `min_w` が無効化され、通常の横スクロールになる
- **`uniform_list` + `Unconstrained` では `flex_1` フィラーが効かない** — 各行の幅はコンテンツで決まるため、`flex_1` は伸びる余地がない。明示的な `min_w` 指定が必要
