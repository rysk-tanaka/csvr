---
paths:
  - "src/**/*.rs"
---

# 設計判断

## 状態変更パターン

`CsvrApp` の状態変更は専用メソッドに集約し、関連する副作用（フィルタ再計算、スクロールリセット等）の呼び忘れを防ぐ。

- `set_search_query()` — クエリ変更 + `refilter_keeping_position`
- `refilter_keeping_position()` — `/` と `&` の入力で `recompute_filtered_indices` に再計算を依頼する。表示位置と選択の維持は `apply_filter_result` が行う
- `toggle_search()` / `close_search()` — 検索バーの表示切替。`close_search` はバーを閉じるがクエリを維持（全コマンド共通の Escape 挙動）
- `toggle_sort(col)` — ソート状態サイクル（None → Asc → Desc → None）。`rows_before_empty` を消し、`pending_scroll_to_top` を立てて `recompute_filtered_indices` に依頼する
- `recompute_filtered_indices()` — `/` フィルタと `&` 正規表現フィルタ（`RowFilter` で 1 回の走査）→ ソートを、`background_executor` のタスクで計算する。`filter_task` を置き換えると前のタスクは捨てられる。`FILTER_CHUNK_ROWS` 行ごとに `yield_now` で処理を譲るので、置き換えたタスクはすぐ止まり、シングルスレッドのブラウザ版でも入力が通る。計算中も表示・選択・統計・チャートは古い結果のまま（コピーやエクスポートが表示と食い違わない）。依頼の時点の表示（`FilterSnapshot`: 並び・先頭位置・選択した元の行）を渡し、ソートを含まない依頼では位置合わせ（`anchored_position` と選択した行の新しい位置）もタスクの中で計算する。並びは `Arc<Vec<usize>>` なのでコピーせずに渡せる
- `apply_filter_result()` — タスクの結果を表示に反映する。選択と統計は必ず消してから戻す。`pending_scroll_to_top` が立っていれば先頭へスクロールし、選択は戻さず、0 件でも `rows_before_empty` を保存しない（ソートの直後に入力してタスクが置き換わっても同じ）。立っていなければ、画面先頭の行以降で最初に残った行を先頭に出す（なければ最後の行）。選択は行が残れば追従し、消えれば解除するだけで、スクロールはしない。GPUI は空のリストのオフセットを 0 に補正するので、0 件になる前の並びと先頭位置を `rows_before_empty` に保持する。`visible_range` は描画でしか更新されないので、次の入力に備えて新しい先頭位置に更新する。計算中にスクロールや選択の変更があってスナップショットと食い違えば、位置合わせは今の表示で計算し直す。選択を戻すときの列は適用の時点の選択から取る（同じ行で列を選び直しても残る）
- `select_cell(filtered_idx, col)` — セル選択。`col=None` で行全体選択
- `clear_selection()` — 選択解除（Escape で呼ばれる）
- `move_selection(row_delta, col_delta, viewport_width)` — 矢印キーによるカーソル移動 + `ensure_visible`（縦）と `ensure_col_visible`（横）で自動スクロール。上下キーでも選択中のセルが見えるように横へ動かす。固定列と行全体の選択では横に動かさない。列変更時のみ `recompute_column_stats` を呼ぶ（行のみ移動時はスキップ）
- `copy_selection(cx)` — 選択中のセル値（またはタブ区切り行）をクリップボードにコピー（`Cmd+C`）
- `toggle_chart()` — チャートパネルの表示/非表示切替（`Cmd+G`）
- `set_chart_type(ct)` — チャートタイプ変更（Bar / Line / Scatter / Histogram）
- `set_chart_col(col)` / `set_chart_x_col(col)` — チャート対象列の変更（数値列のみ）+ `recompute_chart_data`
- `recompute_chart_data()` — `chart_data_cache` を再計算。`toggle_chart`、`set_chart_type`、`set_chart_col`、`set_chart_x_col`、`apply_filter_result` から呼ばれる
- `recompute_column_stats()` — `column_stats_cache` を再計算。`select_cell` と、選択が残ったときの `apply_filter_result` から呼ばれる。`apply_filter_result` ではまずキャッシュを直接 `None` にクリアする
- `toggle_col_filter()` / `set_col_filter_query()` / `close_col_filter()` — `*` コマンドによる列フィルタ。`toggle` はエラーフラグのみ再検証（選択を保持するため `recompute_visible_columns()` は呼ばない）
- `toggle_pin_input()` / `confirm_pin_input()` / `cancel_pin_input()` — `f` コマンドによる列固定。空入力で Enter → `pinned_col_count = 0`（リセット）
- `toggle_row_filter()` / `set_row_filter_query()` / `close_row_filter()` — `&` コマンドによる行の正規表現フィルタ。`toggle` はエラーフラグのみ再検証（選択を保持するため `recompute_filtered_indices()` は呼ばない）。`col:pattern` 構文で列指定可能。入力時は `refilter_keeping_position` で再計算を依頼し、`apply_filter_result` が表示位置を維持する

## ステータスバー

- **読み込み時の警告は表示し続ける** — `CsvData.warnings` を `load_warning` に保持。表示中のデータに関する情報で、起動直後に見逃すことがあるため。stderr には `main` でまとめて出す。推定した文字コード名は `decode_to_utf8` が stderr だけに出す。正しく読めた Shift_JIS の CSV に毎回警告が出ると、本当の警告が目立たなくなるため
- **操作の結果は一時メッセージ** — `show_status_message` で `status_message` に設定し、`STATUS_MESSAGE_DURATION` のあとで消す。タイマーの `Task` を保持して置き換えると前のタイマーが止まるので、新しいメッセージは全期間表示される。表示欄は 1 つで、一時メッセージを優先し、消えると警告に戻る
- **エクスポートはデータがなければクリップボードを書き換えない** — 表示列が空、または表示行が 0 件のときは `No data to export` と表示する。`export_json` などはこの場合も `[]` やヘッダーを返すので、`finish_export` で判定する
- **メッセージと統計は内容の幅に応じて縮める** — どちらも省略表示にする。ウィンドウが狭くても両方の一部が見える。GPUI は `whitespace_nowrap` でも改行で行を分けるので、1 行に保つためにシート名の改行は空白に置き換える
- **統計は `column_stats_cache` にキャッシュ** — `chart_data_cache` と同様のパターン。`select_cell` と、選択が残ったときの `apply_filter_result` でのみ `recompute_column_stats()` で再計算し、それ以外の `apply_filter_result` ではクリアする。`render()` ではキャッシュを参照するだけ（ホバーやリサイズによる再描画で O(n) 計算が走るのを防ぐ）
- **median/stddev の追加で Vec 蓄積が必要に** — median はソートが必須のため単一パスでは不可能。値を `Vec<f64>` に蓄積し、sum/min/max は蓄積と同時に計算。stddev は標本標準偏差（Bessel 補正: `n-1` 除算）、2パス方式（数値安定性のため）。`count == 1` 時は `0.0`。パフォーマンス影響は `recompute_column_stats` がセル選択の変更時と、選択が残っている間のフィルタ入力時にだけ呼ばれることで限定
- **表示フォーマット** — 整数値は小数点なし、それ以外は小数4桁。`format_stat()` ヘルパーで統一。閾値は f64 仮数部の精度限界（2^53 ≈ 9.0e15）

## メタデータヘッダー検出

- **最頻フィールド数でデータヘッダーを自動検出** — `find_dominant_field_count` がヘッダー行を含む全行のフィールド数をカウントし、元ヘッダーのフィールド数より厳密に多い列数が多数派の場合のみ昇格。同票時は元ヘッダーを維持（通常CSVの誤判定を防止）
- **昇格前の行は `CsvData.metadata` に保持** — 元ヘッダー行 + 昇格行より前の全行を `Vec<Vec<String>>` で格納。`from_xlsx` では常に空
- **ヘッダー自動補完** — 昇格後もデータ行がヘッダーより列数が多い場合、`Column N` で補完
- **数値列判定は過半数ベース** — `compute_numeric_columns` は全行一致ではなく非空値の過半数が `f64` にパース可能なら数値列と判定。メタデータ混在行があってもチャートが利用可能
- **メタデータ表示は `m` キーでトグル** — `metadata_visible` フラグで制御。メタデータがある場合は初期表示、ない場合はトグル無効

## ソート

- **数値列判定は全行スキャン・過半数ベース** — `compute_numeric_columns` は初期化時に全行をスキャンし、非空値の過半数が数値ならその列を数値列と判定。フィルタ状態で比較モードが変わるのを防ぐため
- **`f64::total_cmp` を使用** — `partial_cmp` は NaN で `None` を返し全順序を満たさないため。`total_cmp` は NaN に対しても決定的な順序を保証（正の NaN は最大値側に配置）。パース失敗時は `NEG_INFINITY` にフォールバックし最小値側に配置
- **ソートキーは事前計算** — 数値モード時は `Vec<(usize, f64)>` を構築してからソート。`sort_by` 内での O(n log n) 回のパースを回避

## チャート

- **`canvas` 要素で描画** — GPUI の `canvas` は prepaint/paint 2段階のレンダリング。`ChartData` と `ChartAxes` は組にして `chart_data_cache` に保持し、状態変更時のみ `recompute_chart_data()` で再計算。`render()` ではキャッシュを clone して `move` クロージャに渡す
- **ダウンサンプリング** — Bar: 100点、Line/Scatter: 500点に均等間引きで制限。大量データ時のパフォーマンスを確保
- **Scatter の X/Y マッチング** — `extract_scatter_pairs` で1回のイテレーションで両列を同時に抽出。両方の列に有効な数値がある行のみプロット
- **ゼロ除算防止** — 全値同一のときは軸の範囲に幅を持たせる。幅は `1.0` と `|v| * 1e-9` の大きい方で、`1e16` のような値でも `hi > lo` を保つ。Histogram では全値を中央ビンに配置し、横軸はその値を中心にした範囲にする
- **軸の範囲は `compute.rs` で一元化** — `points_axes` / `pairs_axes` / `histogram_axes` が範囲と目盛りのラベルを作り、`draw_chart` はその範囲で描く。目盛りと描画の範囲が食い違わない。`chart` は `compute` に依存しない
- **目盛りは間引き後の値の範囲** — 描いた点と軸を一致させるため。列全体の正確な値はステータスバーの統計で分かる。Histogram の横軸は全データの範囲。Bar/Line の横軸は値ではないので目盛りを付けない
- **目盛りのラベルは隣と区別できる桁数** — `format_tick` が間隔から桁数を決める。Histogram の度数は整数のまま出し、最大が奇数なら中央の目盛りを省く
- **文字は `canvas` の中で描く** — `shape_line` でラベルの幅を測って左の余白を決める。長い列名は `with_content_mask` でチャートの領域に切り取る。`shape_line` は改行を想定しないので、`chart_legend` が改行を空白に置き換える。横軸のラベルは前のラベルと重なるときは描かない（左端は必ず残る）。中央のラベルは右端と重なるときも描かない

## セル選択

- **選択インデックスは `filtered_indices` ベース** — 表示上の位置と一致させることで矢印キー移動が直感的に動作。ソート変更時は選択をクリアし、フィルタ変更時は元の行を探して位置を合わせ、不整合を防ぐ
- **行コピーはタブ区切り** — スプレッドシートへの貼り付け互換性が最も高い
- **`TableRow` に `Entity<CsvrApp>` を保持** — クリックハンドラから親の状態を更新するため。`Entity` は参照カウントされたハンドルなので clone コストは低い
- **ホバーは GPUI の `.hover()` スタイルで実装** — 状態管理不要。行 div に `.id()` を付与して `StatefulInteractiveElement` にし、`.hover(|style| style.bg(...))` で背景色を変更。選択中の行ではホバーを無効化
- **行選択時の矢印キーは非対称** — `col=None`（行全体選択）で右キー→`Some(0)` に遷移してセル選択モードに入る。左キーは `None` のまま（行選択から左に戻す意味がないため）

## 列操作

- **csvlens 互換のインタラクション** — `/`（検索）、`*`（列フィルタ）、`f`（列固定）、`&`（行フィルタ）のプレフィックスコマンド方式。全コマンドで統一された挙動: Enter でバーを閉じてフィルタ維持、Escape でバーを閉じてフィルタ維持（フィルタ解除は再度開いてクエリを削除）
- **入力モードは排他的** — `any_input_active()` で判定。優先順位: `*` > `f` > `&` > `/`。あるモード中は他のトリガーキーや矢印キーを無効化
- **`selected_cell` の col は元のカラムインデックスを保持** — 列非表示でも参照が壊れない。`move_selection` は `visible_col_indices` 内で前後に移動
- **列固定は固定列 div と非固定列 div の分離方式** — GPUI には CSS `position: sticky` がないため、行を「固定部分（行番号 + ピン留め列）」と「スクロール部分（残りの列）」の2つの div に分割。スクロール部分の外側の div を `overflow_hidden` で固定部分の右側にクリップし、内側の div の `ml(h_offset)` で列を動かす。クリップによって表示もヒットボックスも固定部分と重ならない。固定部分の右の境界線（`PINNED_BORDER_WIDTH`、列幅の合計に含める）で、切れた列の値が行番号とつながって見えるのを防ぐ。線を途切れさせないため、上下の余白は行ではなく固定部分とスクロール部分に付ける。`TableRow` は `uniform_list` が行全体をずらすので、固定部分に `ml(-h_offset)` を適用して打ち消す。ヘッダーは `uniform_list` の外にあり、`uniform_list` は prepaint までオフセットを範囲内に戻さないため、`clamp_h_offset` で同じ範囲に収めた値を使う
- **`&` フィルタは `col:pattern` 構文で列指定** — `parse_column_filter` がヘッダー名とコロンを解析。マッチしない場合は全文を全列対象パターンとして扱う
- **`*` と `&` は正規表現ベース** — `regex` クレート使用、case-insensitive。不正パターン時はフォールバック（`*` は全列表示、`&` はフィルタ無適用）

## レイアウト

- **行の幅を明示** — `TableRow` に `row_width: gpui::Pixels`（列幅の合計とビューポート幅の大きい方）を渡し、`w(self.row_width)` を設定。カラム数が少ない場合でも行背景がウィンドウ端まで伸びる。カラム合計幅がビューポートを超える場合は通常の横スクロールになる
- **`uniform_list` + `Unconstrained` では行の幅をコンテンツに任せない** — `uniform_list` は先頭行の幅からスクロールの上限を求めるため、`ml(-h_offset)` が幅の測定に影響すると上限がスクロール量に応じて伸びる。幅を明示すると `h_offset` によらず一定になる
