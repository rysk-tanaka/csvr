# csvr

> CLI から起動する CSV ビューワー。[GPUI](https://github.com/zed-industries/zed/tree/main/crates/gpui)（Zed の UI フレームワーク）で構築。

---

## 概要

csvr はターミナルから CSV ファイルを指定して GUI ウィンドウでテーブル表示するツールです。

- ファイル指定（`csvr data.csv`）またはパイプ入力（`cat data.csv | csvr`）に対応
- 大規模 CSV（数万〜数十万行）でも仮想スクロールで高速に表示
- macOS ネイティブ（Metal レンダリング）

---

## 機能

- [x] CSV 読み込み・テーブル表示
- [x] 列固定ヘッダー
- [x] 列幅の自動調整
- [x] 行番号表示
- [x] インクリメンタル検索・フィルタ（`Cmd+F` / `/`）
- [ ] 列ソート（昇順/降順）

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
