# csvr

[![lint](https://github.com/rysk-tanaka/csvr/actions/workflows/lint.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/lint.yml)
[![test](https://github.com/rysk-tanaka/csvr/actions/workflows/test.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/test.yml)
[![build](https://github.com/rysk-tanaka/csvr/actions/workflows/build.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/build.yml)
[![GPUI](https://img.shields.io/badge/GPUI-v0.188.6-blue)](https://github.com/zed-industries/zed/tree/main/crates/gpui)

> CLI から起動する CSV ビューワー。[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui) で構築。

- ファイル指定（`csvr data.csv`）またはパイプ入力（`cat data.csv | csvr`）に対応
- 大規模 CSV（数万〜数十万行）でも仮想スクロールで高速に表示
- macOS ネイティブ（Metal レンダリング）

## スクリーンショット

| テーブル表示 | 検索 | グラフプレビュー |
| :---: | :---: | :---: |
| ![テーブル表示](docs/images/table.png) | ![検索](docs/images/search.png) | ![グラフプレビュー](docs/images/chart.png) |

---

## 機能

- CSV 読み込み・テーブル表示
- 列固定ヘッダー
- 列幅の自動調整
- 行番号表示
- インクリメンタル検索・フィルタ（`Cmd+F` / `/`）
- 列ソート（昇順/降順） — ヘッダークリックで昇順→降順→解除
- グラフプレビュー（`Cmd+G`） — 棒グラフ・折れ線・散布図・ヒストグラム

---

## セットアップ

GPUI は Metal シェーダーをコンパイルするため、Xcode のフルインストールが必要です。詳細は [docs/setup.md](./docs/setup.md) を参照してください。

### 前提条件

- macOS
- Rust toolchain
- Xcode（Command Line Tools だけでは不足）
- Metal Toolchain

---

## 使い方

```bash
# ファイル指定
csvr data.csv

# パイプ入力
cat data.csv | csvr
```

---

## 開発

```bash
cargo run -- data.csv    # 開発実行
cargo build --release    # リリースビルド
cargo test               # テスト
cargo clippy             # lint
```

---

## 技術スタック

| クレート | 用途 |
| --- | --- |
| [gpui](https://github.com/zed-industries/zed/tree/main/crates/gpui) | UI フレームワーク（Zed） |
| [csv](https://crates.io/crates/csv) | CSV パース |

---

## ライセンス

MIT
