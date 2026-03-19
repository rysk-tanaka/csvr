# CLAUDE.md

## プロジェクト概要

csvr — CLI から起動する CSV ビューワー。GPUI（Zed の UI フレームワーク）で構築する。

## 目標

- `csvr data.csv` または `cat data.csv | csvr` で GUI ウィンドウを開き、CSV をテーブル表示する
- 大規模 CSV（数万〜数十万行）でも仮想スクロールで高速に表示する

## 技術スタック

- Rust
- GPUI（gpui crate）
- csv crate（CSV パース）

## 機能（優先順）

1. ファイル指定またはパイプ入力で CSV を読み込み、テーブル表示
2. 列固定ヘッダー（スクロールしてもヘッダーが残る）
3. 列幅の自動調整
4. 行番号表示
5. インクリメンタル検索・フィルタ
6. 列ソート（昇順/降順）

## 開発コマンド

```bash
cargo run -- data.csv    # 開発実行
cargo build --release    # リリースビルド
cargo test               # テスト
cargo clippy             # lint
```

## 注意事項

- GPUI は macOS 中心のフレームワーク。クロスプラットフォーム対応は初期段階では考慮しない
- GPUI のドキュメントは限られている。Zed のソースコードが最も信頼できるリファレンス
