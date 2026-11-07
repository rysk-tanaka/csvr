# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

csvr — CLI から起動する読み取り専用の CSV ビューワー。GPUI（Zed の UI フレームワーク）で構築。macOS 専用（ブラウザ版は試作。手元では `trunk serve` で起動し、リリースのたびに Cloudflare Workers の <https://csvr.rysk-tanaka.workers.dev> に公開する）。xlsx/xls 読み込みにも対応。編集・保存機能はスコープ外。

## 開発コマンド

```bash
cargo run -- data.csv        # ファイル指定で実行
printf "a,b\n1,2\n" | cargo run  # パイプ入力で実行
cargo test                   # 全テスト実行（default-members でコアも対象）
cargo test test_name         # 単一テスト実行
cargo clippy                 # lint
cargo fmt                    # フォーマット
cargo build --release        # リリースビルド
cargo tarpaulin --skip-clean --out stdout  # カバレッジ計測
cargo check -p csvr-core --target wasm32-unknown-unknown  # コアの wasm32 ビルド確認（要: rustup target add）
CARGO_TARGET_WASM32_WASIP1_RUNNER=wasmtime cargo test -p csvr-core --target wasm32-wasip1  # コアのテストを wasm32 で実行（要: wasmtime）
trunk serve                  # ブラウザ版（試作）を http://127.0.0.1:8080 で起動（要: brew install trunk。release のビルドは Node も要る。設定は Trunk.toml、出力は web/dist）
trunk build --release && npx wrangler dev  # 公開するものを手元で確かめる（_headers も効く。設定は wrangler.jsonc）
cargo clippy --bin csvr --target wasm32-unknown-unknown -- -D warnings  # ブラウザ版の lint（ネイティブの clippy は src/web.rs をビルドしない）
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

cargo workspace の 2 クレート・9 モジュール構成。依存関係: `data`(基盤) ← `compute`, `chart` ← `app` ← `native`, `web` ← `main`。`web` は歓迎画面の `welcome` も使い、`welcome` は `app` を使う。`browser`（ブラウザ版の下回り）はほかのモジュールに依存せず、`app` と `web` が使う。循環依存なし。

- `csvr-core`（`crates/csvr-core`）— `data` と `compute`。GPUI に依存せず、ブラウザ版（gpui_web）と共有するため wasm32 でもビルドできるように保つ。ファイル・stderr・スレッドを使わない
- `csvr`（ルートの `Cargo.toml`）— `main`・`native`・`web`・`welcome`・`browser`・`app`・`chart`。GPUI を使うバイナリ。wasm32 ではブラウザ版（試作）になり、`main` が `native::run` と `web::run` を切り替える。`csvr_core::data` / `csvr_core::compute` を参照する。ルートを bin パッケージのまま残しているのは、`cargo run`・`target/release/csvr`・リリースの version 読み取り（`auto-release.yml`）を変えないため

```text
crates/csvr-core/src/
  lib.rs       -- pub mod data / compute
  data.rs      -- 型定義: CsvData, Table, ChartType, SortDirection, ChartData + テスト
  compute.rs   -- 純粋計算関数 + テスト
src/
  main.rs      -- エントリポイント（native / web の run を呼ぶだけ）
  native.rs    -- ネイティブ版（load_csv, set_dock_icon, run）
  web.rs       -- ブラウザ版（フォントの読み込み、ドロップや選択で開いたファイルの読み込み）
  welcome.rs   -- ブラウザ版のウィンドウのルート `WebRoot`（歓迎画面と表の切り替え、サンプル CSV）
  browser.rs   -- ブラウザ版の下回り（クリップボードへの書き込み、JS のエラー文）。app からも使うので web と分ける
  chart.rs     -- draw_chart + チャートカラー定数
  app.rs       -- CsvrApp, TableRow, UIカラー定数, actions!
```

1. **データ層** (`data.rs`) — `CsvData` が入力パースを担当。CSV は `csv` クレートで `std::io::Read` から読み込み（`flexible(true)` でフィールド数不揃いを許容）、xlsx/xls は `calamine` クレートで読み込み（`from_spreadsheet`。ファイルではなく `Read + Seek` から読む。形式は `SpreadsheetFormat::from_extension` で拡張子から決めて渡し、その形式で開けなければ他の形式も試す）。ヘッダーは `Vec<String>`、行データは `Table`（全セルの文字を 1 本の `String` に行順に詰め、セルの区切り位置と行の開始位置を持つ。CSV では `StringRecord` から直接作る）で保持。メタデータ付きCSV（先頭にkey-value形式のメタデータ行がありデータ本体は途中から始まる形式）では、最頻フィールド数を基にデータヘッダーを自動検出・昇格し、元の行を `metadata` フィールドに保持。`decode_to_utf8` で非 UTF-8 ファイルのエンコーディング自動検出・変換（`chardetng` + `encoding_rs`）。推定した文字コード名は `DecodedInput.decoded_as` で返し、ネイティブ版は `native` が stderr に、ブラウザ版は `web` がコンソールに出す。読み込み時の警告は `CsvData.warnings` に集めてステータスバーと stderr（ネイティブ版）に出す。`ChartType`, `SortDirection`, `ChartData`, `ChartAxes` の型定義
2. **計算層** (`compute.rs`) — テスト可能な純粋関数群。列幅算出（各行の幅は app が実際のフォントで測って渡す）、行フィルタリング（`RowFilter`: 部分一致と正規表現を 1 回の走査で判定し、区切って処理できる。`/` の部分一致はエスケープしたクエリの case-insensitive な正規表現）、数値列判定、ソート、列フィルタ（正規表現で列名マッチ）、チャート用データ抽出・ダウンサンプリング・ヒストグラムビン計算・軸の範囲と目盛り（`points_axes` / `pairs_axes` / `histogram_axes`）・列統計（`ColumnStats`: count/sum/min/max/mean/median/stddev — stddev は標本標準偏差）、エクスポート（`export_json` / `export_markdown` — 表示中データをクリップボードコピー用に変換）
3. **チャート描画** (`chart.rs`) — `draw_chart` 関数。GPUI の `canvas` 要素の paint コールバックから呼ばれる。データに加えて凡例（系列の色と列名）と目盛りも描く
4. **UI 層** (`app.rs`) — `CsvrApp`（`Render` トレイト実装）がメインビュー。`TableRow`（`RenderOnce` / `IntoElement`）が個別行。本体は `uniform_list` による仮想スクロール。カラーテーマは Catppuccin Mocha（`BG_BASE`, `TEXT_MAIN` 等の定数で管理）。`/`（検索）・`*`（列フィルタ）・`f`（列固定）・`&`（行フィルタ）の入力モードは排他制御（`any_input_active()` で判定）。`m`（メタデータ表示トグル）は入力モードではなく単純なトグル

入力の流れ: `load_csv()`（CLI引数 or stdin） → xlsx/xls なら開いたファイルを `CsvData::from_spreadsheet` に渡す、それ以外は `decode_to_utf8` → `CsvData::from_reader` → `CsvrApp::new(data, window, cx)`（列幅を実際のフォントで測る） → GPUI ウィンドウ

ブラウザ版の流れ: `web::run()` → `WebPlatform`（シングルスレッド、日本語は `CanvasFontFallback::EmojiAndCjk` でブラウザのフォントに描かせる）→ IBM Plex Sans（`assets/fonts/ibm-plex-sans`、OFL）を `add_fonts` → `on_missing_glyphs` で足りない字を待つ → ウィンドウのルート `WebRoot` が、何ができるか・開き方・主なキー・ライセンスへのリンクを載せた歓迎画面を出す → `document` の `drop` で受けたファイルや、`OpenFile`（`cmd-o` / `ctrl-o`、歓迎画面の「ファイルを開く」、ステータスバーの Open file）で隠した `<input type="file">` から選んだファイルを、`FileOpener` が読み、`WebRoot::show_table` で `CsvrApp` にして表に切り替える。歓迎画面の「サンプルを開く」はサンプル CSV で同じように切り替える。読み込みは順不同で終わるので、最後に始めたものだけが残るように `WebRoot` が `start_load` で番号を振り、`is_latest_load` で確かめる。サンプルも番号を進めるので、読み込み中のファイルがあとからサンプルを置き換えることはない。開けなかった理由は、歓迎画面ではその画面に、表ではステータスバーに出す。歓迎画面のキーコンテキストは `Welcome` で、`cmd-o` / `ctrl-o` だけを割り当てる。文字の入力欄がないので `!ime_editing` は付けない。gpui_web はドロップされたファイルをアプリに渡さず、ファイルを選ぶ画面も開けないので、`web-sys` で自分で扱う。`OpenFile` は app の要素では受けず、`web` が `cx.on_action` で受ける（app は web を呼べないため）。表では `cmd-o` / `ctrl-o` もほかのキーと同じく IME の変換中は受けない（開くと変換中の文字が消えるため。変換中は gpui_web がどのキーでもブラウザの既定の動作を止めるので、ブラウザ自身の「ファイルを開く」も出ない）。歓迎画面には文字の入力欄がなく、変換中の文字の行き場もないので、変換中でも受ける。選ぶ画面の既定の候補に `.tsv` は入れない（タブ区切りは読めないため）。コピーは `browser::write_clipboard_text` で書き込みの結果を待つ（gpui_web の `write_to_clipboard` は結果を捨てる）。キーは `key_bindings` を `cmd` と `ctrl` の両方で割り当てる（web の `cmd` はどの OS でも Meta キーのため）。どのフォントにもない字が初めて出たら、IBM Plex Sans JP（Regular と SemiBold）を同じサイトの `fonts/` から 2 つ同時に取り、`add_fonts` し、列幅を測り直してから `refresh_windows` で描き直す（`add_fonts` は読めないフォントでも成功を返すが、中身は SRI のハッシュで固定しているので確かめない）。`fonts/` には、Trunk の `post_build` フック（`scripts/web-assets.sh`）が、GitHub のコミットに固定した URL から取ったフォントを入れる（ファイル名とハッシュは `IBM_PLEX_SANS_JP` から読んで確かめ、取ったものは `target/web-fonts/` に残す）。同じフックが `licenses/` にライセンスと、歓迎画面からリンクする一覧の `index.html` を入れ、release のビルドでは最後に `scripts/web-headers.mjs` で `_headers`（CSP とキャッシュ）を書く。Trunk がインラインで書き出すスクリプトはファイル名を含むので、CSP のハッシュをビルドごとに計算する

`set_dock_icon()` は `assets/icon.png` を `include_bytes!` で埋め込み、`objc` クレートで `NSApplication.setApplicationIconImage:` を呼び出して Dock アイコンを設定する。アイコンは Lucide Icons の file-spreadsheet（ISC License、帰属表示は `assets/LICENSE`）を Catppuccin Mocha カラーで調整したもの

## .claude/rules

ソースファイル編集時に自動ロードされるルールファイル:

- `gpui.md` (`src/**/*.rs`) — GPUI API のパターン集・注意点・型対応表
- `design.md` (`src/**/*.rs`, `crates/**/*.rs`) — 状態変更パターン・各機能の設計判断

## Rust Edition

Rust 2024 edition（`edition = "2024"`）を使用。GPUI のバージョンは `Cargo.toml` を参照。

## CI

ビルド・テスト・lint（`build.yml`, `test.yml`, `lint.yml`）は macOS ランナー（`macos-latest`）で実行。GPUI が Metal を必要とするため Linux では不可。ただし `lint.yml` の `cargo fmt --check` ジョブは Metal 不要のため `ubuntu-latest` で実行。これらのワークフローでは `--locked` フラグを使用（Cargo.lock を厳密に再現）。`cargo fmt` は `--locked` 非対応のため除く。テストと clippy は `default-members` でコアも対象になるが、CI では明示のため `--workspace` を付ける。`test.yml` の wasm32 ジョブ（ubuntu-latest）は、コアを `wasm32-unknown-unknown`（gpui_web と同じターゲット）で `cargo check` し、テストを `wasm32-wasip1` で wasmtime を使って実行する（`usize` が 4 バイトになる環境での確認。wasm32-unknown-unknown でテストを動かすには wasm-bindgen-test が要るため）。ブラウザ版は `cargo clippy --bin csvr --target wasm32-unknown-unknown -- -D warnings` で確かめる（`lint.yml` の clippy はネイティブ版だけで、`src/web.rs` をビルドしないため）。

その他のワークフロー（`claude.yml`, `claude-code-review.yml`, `issue-scan.yml`, `issue-implement.yml`）は `rysk-tanaka/workflows` の reusable workflow を呼び出す薄いラッパーで、ubuntu-latest で動作する。`auto-release.yml` は reusable workflow でリリース作成後、macOS ランナーで `aarch64-apple-darwin` / `x86_64-apple-darwin` のバイナリをビルドしリリースにアップロードし、`deploy-web.yml` を呼んでブラウザ版を公開する。

`deploy-web.yml` はブラウザ版を Cloudflare Workers（Static Assets、`wrangler.jsonc`）に出す。本番はリリースのときと、main からの `workflow_dispatch` だけ（GPUI の更新は CI が通れば自動マージされるので、main へのプッシュごとには出さない）。ブラウザ版に関わるファイルに触れた PR では、`wrangler versions upload --preview-alias pr-<番号>` でプレビューを作る（本番は変わらない）。プレビューは GitHub の deployment（環境 `preview`）として PR の head のコミットに記録するので、PR に「View deployment」ボタンが出る（URL はジョブのサマリーにも出す）。そのためデプロイのジョブは `deployments: write` を持ち、呼び出す `auto-release.yml` のジョブにも同じ権限が要る（呼ばれたワークフローは呼び出し側より多くの権限を持てない）。プレビューは PR を閉じても消さないので、workers.dev の URL で誰でも開けるまま残る。フォークと bot（Renovate など）の PR ではプレビューを作らない。ビルド（クレートのビルド用のコードが動く）とデプロイはジョブを分け、Cloudflare のシークレット（`CLOUDFLARE_API_TOKEN`、`CLOUDFLARE_ACCOUNT_ID`）はデプロイのジョブにだけ渡す。trunk はバイナリを sha256 で、wrangler はバージョンで固定している（Renovate は更新しない）。`wrangler.jsonc` の `compatibility_date` は、固定した wrangler が知っている日付より新しくできない

## テスト方針

- GPUI レンダリング（`Render`/`RenderOnce` 実装）はウィンドウ環境が必要なためユニットテスト対象外
- `load_csv` は stdin/プロセス終了に依存するためユニットテスト対象外
- データ処理・レイアウト計算の純粋関数をテスト対象とする（テストは `crates/csvr-core` の `data.rs` と `compute.rs` に配置）

## 既知の設計トレードオフ

- **ブラウザ版（試作）の制約**: `gpui_platform` の既定の feature `multithreaded`（nightly・COOP/COEP が必要）を切り、シングルスレッドで stable のままビルドする。フィルタは区切って処理するので固まらないが、ソートは区切らないので大きいファイルでは終わるまで入力を受け付けない。IBM Plex Sans にない文字のうち、CJK と絵文字はブラウザのフォントで描く（おおよその描画）が、それ以外の字（全角英数字、半角カナ、「〜」「①」「㈱」「▲」など）がデータや入力欄（IME で入力した全角英数字や「〜」も含む）に出ると（データは表示した行のほか、列幅を測る先頭 100 行も含む）、IBM Plex Sans JP の Regular と SemiBold を同じサイトから読み込む（合わせて転送 約 7MB（Workers が brotli で圧縮する）、展開後 約 11MB）。フォントはビルドのときに GitHub から取るので、`trunk serve` や `trunk build` の初回にはネットワークが要る（取れないとき、release のビルドは失敗し、debug のビルドは警告を出してフォントなしで続ける）。URL はコミットに、中身は SRI のハッシュに固定している。読み込むまでは豆腐で、読み込んだあとは日本語（漢字・かな）もこのフォントで描く。ハングルはこのフォントにないので、読み込んだあともブラウザのフォントで描く。歓迎画面の文には、読み込みを起こす字を入れない。使えるのは、gpui_web の `is_cjk` が描く漢字・かな・ハングル・約物と、同梱の IBM Plex Sans にある ASCII やラテン文字、`→` `×` などの字だけで、詳しくは `welcome.rs` の先頭のコメントにある。gpui_web を更新したら見直す。読み込みに失敗したときは豆腐のまま（取りに行くのはページを開いている間に 1 回だけ）。日本語フォントにもない字（アラビア文字やタイ文字など）が出たときも 1 回読み込む。ソートの目印を ↑↓ にしているのは、▲▼ だと IBM Plex Sans にないため、ソートだけで読み込みが起きるから。`/` `*` `&` の入力欄は IME で日本語を入力できる（ネイティブ版も同じ）。gpui_web は見えない textarea で入力を受けるので、`ctrl-b` などでブラウザがカーソルを動かすことがあり、そのときはカーソルを表示した位置で編集する（矢印キーではカーソルを動かせない）。ドロップや選択で開いたファイルの読み込み（文字コードの変換とパース）は区切らずに処理するので、大きいファイルでは終わるまで画面が固まり、読み込み中の表示もない。読み込みの失敗はブラウザのコンソールと画面に出る。表を開いているときはステータスバーに出て数秒で消え、歓迎画面ではその画面に出て、次のファイルかサンプルを開き始めるまで残る。推定した文字コード名はコンソールにだけ出る（`eprintln!` はブラウザでは出ない）。一度に開くのは最初の 1 ファイルだけ。Windows / Linux のブラウザでは `ctrl-shift-j`（Chrome の DevTools）や `ctrl-shift-m`（Firefox のレスポンシブデザインモード）がブラウザに取られることがある。クリップボードへの書き込みは https か localhost でないとできない（失敗はステータスバーに出る）
- **句読点を含む日本語の文の列幅**: 列幅は、ほとんどの行を字と隣り合う 2 字の組の幅から出すが、句読点やかっこ（、。「」（）など）を含む日本語の行は 1 行ずつ丸ごと組んで測る（macOS の CoreText は前後の字によって約物を詰めたり空けたりするので、組だけでは幅が決まらない）。1 行に 10〜40µs かかるので、先頭 100 行の大半が文章の列が多いと開くのが遅くなる（100 行 × 10 列で約 50ms、300 列なら 1 秒ほど）
- **エンコーディング検出とメモリ**: `load_csv` のファイル読み込みは `std::fs::read` で全量バッファリングしてからエンコーディング検出を行う。UTF-8 と判明した場合はバッファを drop しファイルを再オープンしてストリーミング読み込み（再オープン失敗時はバッファにフォールバック）。非 UTF-8 の場合もトランスコード後にバッファを drop。最終メモリ使用量は `CsvData` が全行保持するため変わらないが、ピーク時の同時バッファ保持を回避。行データはセルごとに `String` を持たず `Table` に詰めるが、セルごとに区切り位置（`usize`: ネイティブ版 8 バイト、wasm32 4 バイト）を持つので、小さいセルが多い CSV ではファイルの数倍になる（100 万行 × 10 列・42.5MB の CSV で読み込み後 約 158MB。変更前は約 522MB。wasm32 は未計測）。`Table` の文字列は伸ばすたびに広い領域へ移るので読み込み中は一時的に増えるが、同じ CSV での最大と読み込み後の差は 10MB 未満だった。xlsx は calamine の `Range`（シート全体）と作っている途中の `Table` が同時にメモリにある
- **`/` 検索の大文字小文字の比べ方**: `/` はエスケープしたクエリを case-insensitive な正規表現で比べる（セルごとの `to_lowercase` を避けるため）。Unicode の単純な折りたたみなので、`to_lowercase` とは少し結果が違う（İ は `i` にマッチせず、語末の Σ は `σ` にマッチする）
- **不正なバイトを含む入力の扱い**: UTF-8 の BOM がある場合と、3 バイト以上の文字数が不正箇所の 4 倍以上ある場合は UTF-8 とみなして U+FFFD に置き換える。それ以外は正しい Shift_JIS などと区別できないため文字コードの推定に任せ、推定した文字コード名を stderr に出す。短い破損入力は文字化けすることがある
- **数値列判定の過半数ベースとソート**: `compute_numeric_columns` は非空値の過半数が `f64` にパース可能なら数値列と判定する。これはメタデータ混在CSVでチャートを利用可能にするための変更だが、通常CSVで混在列（例: `E001, 100, 200`）が意図せず数値ソートされ、テキスト値が `NEG_INFINITY` 扱いで最小値側に配置されるリスクがある
