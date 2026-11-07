---
paths:
  - "src/**/*.rs"
  - "crates/**/*.rs"
---

# 設計判断

## 状態変更パターン

`CsvrApp` の状態変更は専用メソッドに集約し、関連する副作用（フィルタ再計算、スクロールリセット等）の呼び忘れを防ぐ。

- `set_search_query()` — クエリ変更 + `needs_refilter` を立てて `recompute_filtered_indices` に依頼
- `toggle_search()` / `close_search()` — 検索バーの表示切替。`close_search` はバーを閉じるがクエリを維持（全コマンド共通の Escape 挙動）
- `toggle_sort(col)` — ソート状態サイクル（None → Asc → Desc → None）。`rows_before_empty` を消し、`pending_scroll_to_top` を立てて `recompute_filtered_indices` に依頼する
- `recompute_filtered_indices()` — `background_executor` のタスクで、`/` フィルタと `&` 正規表現フィルタ（`RowFilter` で 1 回の走査）→ ソートを計算する。`needs_refilter` が立っていなければ（ソートだけの変更）、表示中の行を `reorder_indices` で並べ替えるだけにする。並べ替えはファイル順から始めるので、同じ値の行はフィルタし直したときと同じくファイル順になる（前のソート順を引き継がない）。ソート解除はファイル順に戻す。`filter_task` を置き換えると前のタスクは捨てられる。`FILTER_CHUNK_ROWS` 行ごとと、ソートの後に `yield_now` で処理を譲るので、置き換えたタスクはそこで止まり、シングルスレッドで動くブラウザ版（gpui_web）でも入力が通る（ソート自体は一括）。計算中も表示・選択・統計・チャートは古い結果のまま（コピーが表示と食い違わない）。計算中は件数の表示に `(filtering...)` を付け、エクスポートは受け付けない（`is_ready_to_export`）。古い結果を絞り込み後のデータと取り違えて書き出すのを防ぐため。フィルタし直すときは表示中の並びをタスクに渡さない（`RowSource`）。`RowSource::Filter` はクエリを文字列のまま渡し、`RowFilter` はタスクの中で作る。長いクエリを貼ったときに、`/` の正規表現の作成でキー入力ごとに UI スレッドが止まらないようにするため。`&` のパターンは、エラーを結果を待たずに出すため `is_valid_pattern` で UI スレッドでも 1 回作る（タスクの中でもう 1 回作る）。スクロールの扱い（`ScrollTarget`）は依頼の時点で決める。`pending_scroll_to_top` なら `Top`、そうでなければ元の行→新しい位置の対応表（`RowPositions`、全行数ぶんの `u32`）もタスクの中で作って `Keep` にする。表示中の並びは `Arc<Vec<usize>>` なので、ソートだけの変更でもコピーせずに渡せる
- `apply_filter_result()` — タスクの結果を表示に反映し、`filter_task` を外す（計算中かどうかは `is_filtering` がこれで判定する）。位置合わせは結果が届いた時点の表示（`anchor_view` の並びと先頭位置、選択）に対して、`RowPositions` を引くだけで行う。計算中にスクロールや選択の変更があっても計算し直さずに済み、UI スレッドで全件を走査しない。選択と統計は必ず消してから戻し、`pending_scroll_to_top` を下ろし、フィルタし直した結果なら `needs_refilter` も下ろす。`Top` なら先頭へスクロールし、選択は戻さず、0 件でも `rows_before_empty` を保存しない（ソートの直後に入力してタスクが置き換わっても同じ）。`Keep` なら、画面先頭の行以降で最初に残った行を先頭に出す（なければ最後の行）。選択は行が残れば追従し（列は今の選択のまま）、消えれば解除するだけで、スクロールはしない。GPUI は空のリストのオフセットを 0 に補正するので、0 件になる前の並びと先頭位置を `rows_before_empty` に保持する。`visible_range` は描画でしか更新されないので、次の入力に備えて新しい先頭位置に更新する
- `select_cell(filtered_idx, col)` — セル選択。`col=None` で行全体選択
- `clear_selection()` — 選択解除（Escape で呼ばれる）
- `move_selection(row_delta, col_delta, viewport_width)` — 矢印キーによるカーソル移動 + `ensure_visible`（縦）と `ensure_col_visible`（横）で自動スクロール。上下キーでも選択中のセルが見えるように横へ動かす。固定列と行全体の選択では横に動かさない。列変更時のみ `recompute_column_stats` を呼ぶ（行のみ移動時はスキップ）
- `copy_selection(cx)` — 選択中のセル値（またはタブ区切り行）をクリップボードにコピー（`Cmd+C`）。書き込みはエクスポートと同じ `write_clipboard` を通す
- `toggle_chart()` — チャートパネルの表示/非表示切替（`Cmd+G`）
- `set_chart_type(ct)` — チャートタイプ変更（Bar / Line / Scatter / Histogram）
- `set_chart_col(col)` / `set_chart_x_col(col)` — チャート対象列の変更（数値列のみ）+ `recompute_chart_data`
- `recompute_chart_data()` — `chart_data_cache` を再計算。`toggle_chart`、`set_chart_type`、`set_chart_col`、`set_chart_x_col`、`apply_filter_result` から呼ばれる
- `recompute_column_stats()` — `column_stats_cache` を再計算。`select_cell` と、選択が残ったときの `apply_filter_result` から呼ばれる。`apply_filter_result` ではまずキャッシュを直接 `None` にクリアする
- `toggle_col_filter()` / `set_col_filter_query()` / `close_col_filter()` — `*` コマンドによる列フィルタ。`toggle` はエラーフラグのみ再検証（選択を保持するため `recompute_visible_columns()` は呼ばない）
- `toggle_pin_input()` / `confirm_pin_input()` / `cancel_pin_input()` — `f` コマンドによる列固定。空入力で Enter → `pinned_col_count = 0`（リセット）
- `toggle_row_filter()` / `set_row_filter_query()` / `close_row_filter()` — `&` コマンドによる行の正規表現フィルタ。`toggle` はエラーフラグのみ再検証（選択を保持するため `recompute_filtered_indices()` は呼ばない）。`col:pattern` 構文で列指定可能。入力時は `needs_refilter` を立てて `recompute_filtered_indices` に依頼し、`apply_filter_result` が表示位置を維持する。エラーフラグは結果を待たずに入力の時点で判定する（バーを閉じた後に結果が来ても立ち直らない）。正規表現の設定は `RowFilter::build_regex` の 1 か所で、検証（`RowFilter::is_valid_pattern`）とフィルタで食い違わない
- `text_input`（`TextInputState`）/ `commit_text_input()` — `/` `*` `&` のバーで編集中の文字。IME の変換中の文字（marked range）も含み、バーを開くときに今のクエリから作り、閉じると消す。各クエリ（`search_query` など）は最後に確定した文字列のままにし、IME が編集していない（`is_ime_editing()` が false）ときだけ `commit_text_input` で setter に入れ直す。変換中のローマ字の途中や再変換の途中でフィルタが走らないようにするため。`is_ime_editing` は web では `is_composition_open` も見る（gpui_web は空の `compositionupdate` のあとも `compositionend` まで変換を続け、その間もキーが届くため）。macOS では見ない（空の `setMarkedText` だけで変換を終えることがあり、印が残るため）
- `CsvrApp` の `EntityInputHandler` — `text_input` を GPUI の入力の受け口として渡す。`text_input_line` が paint の中で `handle_input` で登録する。選択の変更（`set_selected_text_range`）はいつでも受ける。gpui_web は textarea の選択を取り込んだ前提で次の編集の範囲を決めるので、受けないと食い違って編集の位置がずれるため。そのためカーソルと選択範囲を表示する

## ステータスバー

- **読み込み時の警告は表示し続ける** — `CsvData.warnings` を `load_warning` に保持。表示中のデータに関する情報で、起動直後に見逃すことがあるため。stderr にはネイティブ版の `native::run` でまとめて出す。推定した文字コード名は `decode_to_utf8` が `DecodedInput.decoded_as` で返し、ネイティブ版は stderr だけに、ブラウザ版はコンソールだけに出す（コアは表示先を知らないので出力しない）。正しく読めた Shift_JIS の CSV に毎回警告が出ると、本当の警告が目立たなくなるため
- **ブラウザ版でファイルを開けなかったら知らせる** — 読み込みや解析に失敗したら、`web` の `FileOpener::report_failure` が `WebRoot::show_message` に渡し、コンソールにも出す。表なら `show_status_message` で一時メッセージにし、歓迎画面なら、画面は歓迎画面のままで、次のファイルかサンプルを開き始めるまでその画面に出す（`start_load` が消す。残すと、次のファイルを読んでいる間もそのファイルが失敗したように見えるため）。ページに置いたファイルが開かないと、何も起きなかったように見えるため。後のファイルに置き換えられたファイルの失敗はコンソールにだけ出す（後のファイルの表示を覆わないため）
- **ブラウザ版のコピーは結果を待って知らせる** — `write_clipboard` は、ネイティブ版では書き込んですぐにメッセージを出す。ブラウザ版は `browser::write_clipboard_text` の結果を切り離したタスクで待ち、成功ならメッセージ、失敗なら `Copy failed: <理由>` を出す（続けてコピーしても、前のコピーの結果は捨てない）。ブラウザは https か localhost でないと `navigator.clipboard` がなく、許可がないと拒否するが、gpui_web の `write_to_clipboard` はどちらも知らせない。書き込みはキー入力のイベントの中で始める（ブラウザがユーザー操作の中でだけ許すため）
- **操作の結果は一時メッセージ** — `show_status_message` で `status_message` に設定し、`STATUS_MESSAGE_DURATION` のあとで消す。タイマーの `Task` を保持して置き換えると前のタイマーが止まるので、新しいメッセージは全期間表示される。表示欄は 1 つで、一時メッセージを優先し、消えると警告に戻る
- **エクスポートはデータがなければクリップボードを書き換えない** — 表示列が空、または表示行が 0 件のときは `No data to export` と表示する。`export_json` などはこの場合も `[]` やヘッダーを返すので、`finish_export` で判定する
- **メッセージと統計は内容の幅に応じて縮める** — どちらも省略表示にする。ウィンドウが狭くても両方の一部が見える。GPUI は `whitespace_nowrap` でも改行で行を分けるので、1 行に保つためにシート名の改行は空白に置き換える
- **統計は `column_stats_cache` にキャッシュ** — `chart_data_cache` と同様のパターン。`select_cell` と、選択が残ったときの `apply_filter_result` でのみ `recompute_column_stats()` で再計算し、それ以外の `apply_filter_result` ではクリアする。`render()` ではキャッシュを参照するだけ（ホバーやリサイズによる再描画で O(n) 計算が走るのを防ぐ）
- **median/stddev の追加で Vec 蓄積が必要に** — median はソートが必須のため単一パスでは不可能。値を `Vec<f64>` に蓄積し、sum/min/max は蓄積と同時に計算。stddev は標本標準偏差（Bessel 補正: `n-1` 除算）、2パス方式（数値安定性のため）。`count == 1` 時は `0.0`。パフォーマンス影響は `recompute_column_stats` がセル選択の変更時と、選択が残っている間のフィルタ入力時にだけ呼ばれることで限定
- **表示フォーマット** — 整数値は小数点なし、それ以外は小数4桁。`format_stat()` ヘルパーで統一。閾値は f64 仮数部の精度限界（2^53 ≈ 9.0e15）

## 行データ

- **`Table` に行順に詰めて持つ** — 全セルの文字を 1 本の `String` に区切りなしで並べ、セルの終わりの位置（`cell_bounds`）と行の最初のセルの番号（`row_starts`）を持つ。セルごとに `String` を持つと 1 セルで 40 バイト以上使い、ファイルの数倍のメモリになる。ブラウザ版（gpui_web）は wasm のメモリ上限があり、ネイティブ版では開けるファイルが開けなくなるため
- **列ごとではなく行順** — フィールド数が不揃いな CSV では、列ごとの形だと幅の広い行が 1 つあるだけで、増えた列すべてに全行ぶんの区切りが要る。行順なら行ごとの幅をそのまま表せる。ソート・統計・チャートは行番号でセルを引くので、列を連続して読める利点はあまり効かない。外には `row` / `cell` / `row_count` などの API だけを見せるので、形を変えても使う側は変わらない
- **区切りは `usize`** — wasm32 では 4 バイトになり、あふれの処理が要らない。ネイティブ版の 8 バイトでも、セルごとの `String` と比べれば小さい
- **足りないセルは `None`** — `cell(r, c)` はその行のセル数を超えると `None` を返す（`Vec::get` と同じ）。空のフィールドは `Some("")`。行全体のコピーは `None` のセルを飛ばす
- **不変条件は `Table` の中だけで守る** — 変更するのは `push_record` / `push_row` / `drop_leading_rows` だけで、`debug_check` で確かめる。`push_record` は `StringRecord::as_slice` をそのまま追加するので、フィールドごとの `String` を作らない
- **`Arc<Table>` で共有** — 読み込み後は変更しないので、フィルタのタスクと `TableRow` に `Arc` の clone だけで渡せる。`TableRow` は表と行番号を持ち、見えるセルだけ `SharedString` にする
- **先頭行の削除はその場で行う** — ヘッダーの昇格で `drop_leading_rows` が `drain` で詰め、残りの区切りから削った分を引く。コピーをもう 1 つ作らない。`column_count` は残りの行から計算し直す（`Column N` の補完は残りの行だけを数えるため）

## メタデータヘッダー検出

- **最頻フィールド数でデータヘッダーを自動検出** — `find_dominant_field_count` がヘッダー行を含む全行のフィールド数（`Table::row_lens`）をカウントし、元ヘッダーのフィールド数より厳密に多い列数が多数派の場合のみ昇格。同票時は元ヘッダーを維持（通常CSVの誤判定を防止）
- **昇格前の行は `CsvData.metadata` に保持** — 元ヘッダー行 + 昇格行より前の全行を `Vec<Vec<String>>` で格納。行数が少ないので `Table` にはしない。読み込みでは全行を `Table` に入れてから、昇格した行までを取り出して `drop_leading_rows` で消す。`from_spreadsheet` では常に空
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
- **`/` `*` `&` の文字は入力の受け口で受ける** — IME の変換中の文字と確定した文字を受けるため。`handle_text_input_key` は文字を足さず（足すと 2 回入る）、Enter・Backspace・全選択（`cmd-a`。ブラウザ版は `ctrl-a` も。macOS の `ctrl-a` は行頭への移動なので、ネイティブ版では受けない）と矢印キーなどだけを処理して伝播を止める。全選択をアプリで処理するのは、gpui_web の textarea には選択の前後 512 UTF-16 単位しか写さず、ブラウザの全選択では長いクエリの全体が選ばれないため。矢印キーではカーソルを動かさない（macOS の GPUI は動かさないので、版で挙動を揃える）。IME の変換中はキーを処理せず、キーの割り当て（Escape の `DismissSearch`、`cmd-f` など）もすべて外す（変換中にバーが閉じると、変換中の文字の行き場がなくなるため）。外すのは `key_bindings` のコンテキスト `CsvrApp && !ime_editing` の 1 か所で、`render` が変換中だけキーコンテキストに `ime_editing` を足す。GPUI は次のキーを処理する前に変わったビューを描き直すので、コンテキストは最新になる。アクションを足すときに、変換中のチェックを書き忘れないようにするため。通常モードで入力欄を開くトリガーキーは伝播を止める。直前に閉じた入力欄の受け口は次の描画まで残っていて、止めないとトリガーキーの文字が開いた入力欄に入るため
- **バーの文字は `canvas` で 1 行に組んで描く** — カーソルと選択範囲を文字の位置に合わせて描き、長い入力でもカーソルが見えるように横にずらす（`caret_scroll_offset`）。`TextRun` は変換中の文字の前後でだけ分け（下線）、選択範囲は別に塗るので、両者が重なっても run が重ならない。改行は `TextInputState` が空白に置き換える（`shape_line` は改行を受けない）
- **`selected_cell` の col は元のカラムインデックスを保持** — 列非表示でも参照が壊れない。`move_selection` は `visible_col_indices` 内で前後に移動
- **列固定は固定列 div と非固定列 div の分離方式** — GPUI には CSS `position: sticky` がないため、行を「固定部分（行番号 + ピン留め列）」と「スクロール部分（残りの列）」の2つの div に分割。スクロール部分の外側の div を `overflow_hidden` で固定部分の右側にクリップし、内側の div の `ml(h_offset)` で列を動かす。クリップによって表示もヒットボックスも固定部分と重ならない。固定部分の右の境界線（`PINNED_BORDER_WIDTH`、列幅の合計に含める）で、切れた列の値が行番号とつながって見えるのを防ぐ。線を途切れさせないため、上下の余白は行ではなく固定部分とスクロール部分に付ける。`TableRow` は `uniform_list` が行全体をずらすので、固定部分に `ml(-h_offset)` を適用して打ち消す。ヘッダーは `uniform_list` の外にあり、`uniform_list` は prepaint までオフセットを範囲内に戻さないため、`clamp_h_offset` で同じ範囲に収めた値を使う
- **`&` フィルタは `col:pattern` 構文で列指定** — `parse_column_filter` がヘッダー名とコロンを解析。マッチしない場合は全文を全列対象パターンとして扱う
- **`*` と `&` は正規表現ベース** — `regex` クレート使用、case-insensitive。不正パターン時はフォールバック（`*` は全列表示、`&` はフィルタ無適用）
- **`/` は文字どおりの部分一致** — `regex::escape` したクエリを case-insensitive の正規表現にする（`SearchQuery`）。セルごとに `to_lowercase` で `String` を作らずに済むため。大文字小文字は Unicode の単純な折りたたみで比べるので、`to_lowercase` とは結果が少し違う（İ は `i` にマッチしなくなり、語末の Σ は `σ` にマッチするようになる）。クエリが長すぎて正規表現を作れないときだけ、`to_lowercase` で比べる

## レイアウト

- **行の幅を明示** — `TableRow` に `row_width: gpui::Pixels`（列幅の合計とビューポート幅の大きい方）を渡し、`w(self.row_width)` を設定。カラム数が少ない場合でも行背景がウィンドウ端まで伸びる。カラム合計幅がビューポートを超える場合は通常の横スクロールになる
- **列幅は実際のフォントで測る** — `compute_column_widths` は先頭 100 行とヘッダーを改行で分け、一番広い行に余白を足して上下限に収める。各行の幅は app が描くときのフォントで測る関数を渡す（コアはフォントを知らない）。セルの同じ文字列はコアのキャッシュで 1 回だけ測る（ヘッダーはキャッシュしない）。セルは `LineMeasure` が、字の幅と、隣り合う 2 字の組の差（カーニングや、CoreText が漢字・かなと英字・半角カナの間に空ける約 1.75px）を足して出す。漢字・かなどうしの組は差が 0 なので組まない（`pair_needs_shaping`。日本語の文章は組の種類が多く、全部組むと行を丸ごと組むより遅くなる）。列幅は 400px（`MAX_COL_WIDTH`）で頭打ちなので、行がその幅（余白を引いて 376px）に達したら測るのをやめる。その幅はコアが測る関数に渡し、app はコアの定数を見ない。丸ごと組む行も、長ければ先頭 256 字を組み、その幅に達していれば残りを組まない（ヘッダーも同じ）。1 行を組むのに 10〜40µs かかり、300 列のファイルでは開くのに 1 秒ほど余分にかかったため。字と組はファイル全体で 1 回ずつ組むので、300 列でも数十 ms で済む。丸ごと組んだ幅との差は、ネイティブ版（CoreText）では全角英数字・半角カナの混ざった作為的な行の -0.6px が最大で、ほかは 0.01px 未満だった。ブラウザ版（cosmic-text）では約 3 万行のすべてで 0〜+0.42px（広く出る側だけ）だった。2 字の組では幅が決まらない字を含む行は、`measurable_by_pairs` で見分けて丸ごと組む。VS16 の `©️`（絵文字になり約 2 倍）、ZWJ の絵文字、結合文字、アラビア文字などと、漢字・かなの約物（、。「」（）など。CoreText が前後の字によって詰めたり空けたりするので、組だけで測ると日本語の文章の 1 行で 28px ずれた）とハングル（漢字・かなと混ざると 10px ずれた）。ヘッダーは数が少ないので丸ごと組み、太字の `text_xs` で測ってソートの目印（`ml_0p5` と ↑↓ の広い方）の幅を常に足す。ソートしても列幅が変わらず表がずれない。行番号の列は一番広い数字の幅で決める。ブラウザ版は日本語フォントを読み込んだら測り直す（`remeasure_columns`。読み込む前は足りない字を豆腐の幅で測っている）
- **`uniform_list` + `Unconstrained` では行の幅をコンテンツに任せない** — `uniform_list` は先頭行の幅からスクロールの上限を求めるため、`ml(-h_offset)` が幅の測定に影響すると上限がスクロール量に応じて伸びる。幅を明示すると `h_offset` によらず一定になる
