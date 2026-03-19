# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## プロジェクト概要

csvr — CLI から起動する CSV ビューワー。GPUI（Zed の UI フレームワーク）で構築。macOS 専用。

## 開発コマンド

```bash
cargo run -- data.csv        # ファイル指定で実行
echo "a,b\n1,2" | cargo run  # パイプ入力で実行
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
2. **レイアウト計算** — `compute_column_widths`（先頭100行サンプリングで列幅算出）、`row_number_col_width`（行番号列の桁幅算出）。テスト可能な純粋関数
3. **UI 層** — `CsvrApp`（`Render` トレイト実装）がメインビュー。`TableRow`（`RenderOnce` / `IntoElement`）が個別行。本体は `uniform_list` による仮想スクロール

入力の流れ: `load_csv()`（CLI引数 or stdin） → `CsvData` → `CsvrApp::new()` → GPUI ウィンドウ

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

### 重要な型の対応

- `App` — アプリケーションコンテキスト（旧 `AppContext`）
- `Context<T>` — ビューコンテキスト（旧 `ViewContext<T>`）
- `Entity<T>` — ビューハンドル（旧 `View<T>`）
- `UniformListScrollHandle` — uniform_list のスクロール状態管理

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
5. インクリメンタル検索・フィルタ
6. 列ソート（昇順/降順）
