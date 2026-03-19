# csvr セットアップガイド

GPUI（Zed の UI フレームワーク）を使用するため、通常の Rust プロジェクトよりセットアップが多い。

## 前提条件

- macOS（GPUI は macOS 中心のフレームワーク）
- Rust toolchain（rustup 経由）
- Git（SSH または HTTPS で GitHub にアクセス可能であること）

## 1. Xcode のインストール

GPUI は Metal シェーダーをコンパイルするため、**Command Line Tools だけでは不足**。Xcode のフルインストールが必要。

```bash
# Mac App Store からインストール（mas 経由）
mas install 497799835

# 開発ツールのパスを Xcode に切り替え
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer

# ライセンスに同意
sudo xcodebuild -license accept

# 初回起動セットアップ（CoreSimulator 等のインストール）
xcodebuild -runFirstLaunch
```

## 2. Metal Toolchain のインストール

Xcode をインストールしても Metal Toolchain は別途必要な場合がある。

```bash
# CLI でインストールを試す
xcodebuild -downloadComponent MetalToolchain
```

上記が失敗する場合は、Xcode を GUI で起動し **Settings > Components** から Metal Toolchain をダウンロードする。

```bash
open /Applications/Xcode.app
```

### 確認方法

```bash
xcrun metal --version
# 正常時: Apple metal version XXXXX ... と表示される
```

## 3. ビルド

```bash
cargo check    # コンパイルチェック
cargo run      # 開発実行
```

## トラブルシューティング

| エラー | 原因 | 対処 |
| -------- | ------ | ------ |
| `unable to find utility "metal"` | xcode-select が Command Line Tools を指している | `sudo xcode-select -s /Applications/Xcode.app/Contents/Developer` |
| `You have not agreed to the Xcode license` | Xcode ライセンス未同意 | `sudo xcodebuild -license accept` |
| `cannot execute tool 'metal' due to missing Metal Toolchain` | Metal Toolchain 未インストール | Xcode の Settings > Components からインストール |
| `failed to authenticate when downloading repository` | Cargo の git fetch が SSH 認証に失敗 | `.cargo/config.toml` に `[net] git-fetch-with-cli = true` を追加 |
