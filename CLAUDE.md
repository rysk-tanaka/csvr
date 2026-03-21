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
2. **計算層** (`compute.rs`) — テスト可能な純粋関数群。列幅算出、行フィルタリング（部分一致・正規表現）、数値列判定、ソート、列フィルタ（正規表現で列名マッチ）、チャート用データ抽出・ダウンサンプリング・ヒストグラムビン計算・列統計（`ColumnStats`: count/sum/min/max/mean/median/stddev — stddev は標本標準偏差）など
3. **チャート描画** (`chart.rs`) — `draw_chart` 関数。GPUI の `canvas` 要素の paint コールバックから呼ばれる
4. **UI 層** (`app.rs`) — `CsvrApp`（`Render` トレイト実装）がメインビュー。`TableRow`（`RenderOnce` / `IntoElement`）が個別行。本体は `uniform_list` による仮想スクロール

入力の流れ: `load_csv()`（CLI引数 or stdin） → `CsvData` → `CsvrApp::new(data, cx)` → GPUI ウィンドウ

## .claude/rules

ソースファイル編集時に自動ロードされるルールファイル:

- `gpui.md` (`src/**/*.rs`) — GPUI API のパターン集・注意点・型対応表
- `design.md` (`src/**/*.rs`) — 状態変更パターン・各機能の設計判断

## Rust Edition

Rust 2024 edition（`edition = "2024"`）を使用。GPUI v0.188.6 との互換性は確認済み。

## CI

GitHub Actions は macOS ランナー（`macos-latest`）で実行。GPUI が Metal を必要とするため Linux では clippy/test/build いずれも不可。全ワークフローで `--locked` フラグを使用（Cargo.lock を厳密に再現）。

## テスト方針

- GPUI レンダリング（`Render`/`RenderOnce` 実装）はウィンドウ環境が必要なためユニットテスト対象外
- `load_csv` は stdin/プロセス終了に依存するためユニットテスト対象外
- データ処理・レイアウト計算の純粋関数をテスト対象とする（テストは `data.rs` と `compute.rs` に配置）
