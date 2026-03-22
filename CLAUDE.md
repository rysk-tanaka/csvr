# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

csvr — CLI から起動する読み取り専用の CSV ビューワー。GPUI（Zed の UI フレームワーク）で構築。macOS 専用。xlsx/xls 読み込みにも対応。編集・保存機能はスコープ外。

## 開発コマンド

```bash
cargo run -- data.csv        # ファイル指定で実行
printf "a,b\n1,2\n" | cargo run  # パイプ入力で実行
cargo test                   # 全テスト実行
cargo test test_name         # 単一テスト実行
cargo clippy                 # lint
cargo fmt                    # フォーマット
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

1. **データ層** (`data.rs`) — `CsvData` が入力パースを担当。CSV は `csv` クレートで `std::io::Read` から読み込み、xlsx/xls は `calamine` クレートで読み込み（`from_xlsx`）。ヘッダーと行データを `Vec<String>` で保持。`decode_to_utf8` で非 UTF-8 ファイルのエンコーディング自動検出・変換（`chardetng` + `encoding_rs`）。`ChartType`, `SortDirection`, `ChartData` の型定義
2. **計算層** (`compute.rs`) — テスト可能な純粋関数群。列幅算出、行フィルタリング（部分一致・正規表現）、数値列判定、ソート、列フィルタ（正規表現で列名マッチ）、チャート用データ抽出・ダウンサンプリング・ヒストグラムビン計算・列統計（`ColumnStats`: count/sum/min/max/mean/median/stddev — stddev は標本標準偏差）、エクスポート（`export_json` / `export_markdown` — 表示中データをクリップボードコピー用に変換）
3. **チャート描画** (`chart.rs`) — `draw_chart` 関数。GPUI の `canvas` 要素の paint コールバックから呼ばれる
4. **UI 層** (`app.rs`) — `CsvrApp`（`Render` トレイト実装）がメインビュー。`TableRow`（`RenderOnce` / `IntoElement`）が個別行。本体は `uniform_list` による仮想スクロール。カラーテーマは Catppuccin Mocha（`BG_BASE`, `TEXT_MAIN` 等の定数で管理）。`/`（検索）・`*`（列フィルタ）・`f`（列固定）・`&`（行フィルタ）の入力モードは排他制御（`any_input_active()` で判定）

入力の流れ: `load_csv()`（CLI引数 or stdin） → xlsx なら `CsvData::from_xlsx`、それ以外は `decode_to_utf8` → `CsvData::from_reader` → `CsvrApp::new(data, cx)` → GPUI ウィンドウ

## .claude/rules

ソースファイル編集時に自動ロードされるルールファイル:

- `gpui.md` (`src/**/*.rs`) — GPUI API のパターン集・注意点・型対応表
- `design.md` (`src/**/*.rs`) — 状態変更パターン・各機能の設計判断

## Rust Edition

Rust 2024 edition（`edition = "2024"`）を使用。GPUI v0.188.6 との互換性は確認済み。

## CI

ビルド・テスト・lint（`build.yml`, `test.yml`, `lint.yml`）は macOS ランナー（`macos-latest`）で実行。GPUI が Metal を必要とするため Linux では不可。ただし `lint.yml` の `cargo fmt --check` ジョブは Metal 不要のため `ubuntu-latest` で実行。これらのワークフローでは `--locked` フラグを使用（Cargo.lock を厳密に再現）。`cargo fmt` は `--locked` 非対応のため除く。

その他のワークフロー（`claude.yml`, `claude-code-review.yml`, `issue-scan.yml`, `issue-implement.yml`）は `rysk-tanaka/workflows` の reusable workflow を呼び出す薄いラッパーで、ubuntu-latest で動作する。`auto-release.yml` は reusable workflow でリリース作成後、macOS ランナーで `aarch64-apple-darwin` / `x86_64-apple-darwin` のバイナリをビルドしリリースにアップロードする。

## テスト方針

- GPUI レンダリング（`Render`/`RenderOnce` 実装）はウィンドウ環境が必要なためユニットテスト対象外
- `load_csv` は stdin/プロセス終了に依存するためユニットテスト対象外
- データ処理・レイアウト計算の純粋関数をテスト対象とする（テストは `data.rs` と `compute.rs` に配置）

## 既知の設計トレードオフ

- **エンコーディング検出とメモリ**: `load_csv` のファイル読み込みは `std::fs::read` で全量バッファリングしてからエンコーディング検出を行う。UTF-8 の場合でもファイル全体の検証が必要なため、ストリーミング読み込みとの両立が課題。ピークメモリ最適化は #21 で別途対応予定
- **`decode_to_utf8` の `Some(Err)` パス**: `encoding_rs` の `had_errors == true` は検出エンコーディングで無効なバイト列がある場合に発生するが、`chardetng` は短いバイト列を Windows-1252 等の全バイト値を許容するエンコーディングに推定しやすい。そのため `Some(Err)` パスをテストで確実にトリガーする入力が作りにくく、`decode_non_utf8_attempts_transcoding` テストは「panic しないこと」と「`Some` が返ること」のみを保証する
