# GitHub Actions ワークフロー一覧

このディレクトリにあるワークフローを用途別に整理した一覧です。

## 外部ワークフロー参照

汎用的なワークフローは [rysk-tanaka/workflows](https://github.com/rysk-tanaka/workflows) リポジトリに移設しています。

| 外部リソース | 用途 |
| --- | --- |
| `release-on-version-change.yml` | バージョン変更時のタグ作成と Release 作成 |
| `claude.yml` | `@claude` メンション応答の共通ロジック |
| `claude-code-review.yml` | コードレビューの共通ロジック |
| `issue-scan.yml` | Issue トリアージの共通ロジック |
| `issue-implement.yml` | Issue 自動実装の共通ロジック |
| `resolve-version` (action) | プロジェクトファイルからバージョン検出 |
| `release-core` (action) | タグ・Release 作成（冪等） |

## CI と品質チェック

GPUI が Metal を必要とするため、macOS ランナーで実行。全ワークフローで `--locked` フラグを使用。

| Workflow | Status | 主目的 | トリガー |
| --- | --- | --- | --- |
| [lint.yml](./lint.yml) | [![Lint](https://github.com/rysk-tanaka/csvr/actions/workflows/lint.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/lint.yml) | `cargo clippy` の実行 | `push` (main), `pull_request` (main) |
| [test.yml](./test.yml) | [![Test](https://github.com/rysk-tanaka/csvr/actions/workflows/test.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/test.yml) | `cargo test` の実行 | `push` (main), `pull_request` (main) |
| [build.yml](./build.yml) | [![Build](https://github.com/rysk-tanaka/csvr/actions/workflows/build.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/build.yml) | リリースビルドの確認 | `push` (main), `pull_request` (main) |

## リリース

| Workflow | Status | 主目的 | トリガー |
| --- | --- | --- | --- |
| [auto-release.yml](./auto-release.yml) | [![Release](https://github.com/rysk-tanaka/csvr/actions/workflows/auto-release.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/auto-release.yml) | `Cargo.toml` のバージョン変更で GitHub Release を作成 | `push` (main, `Cargo.toml`), `workflow_dispatch` |

## Issue 自動化

| Workflow | Status | 主目的 | トリガー |
| --- | --- | --- | --- |
| [issue-scan.yml](./issue-scan.yml) | [![Issue Scan](https://github.com/rysk-tanaka/csvr/actions/workflows/issue-scan.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/issue-scan.yml) | open issue の難易度判定とラベル付与 | `schedule` (`50 0 * * *`), `workflow_dispatch` |
| [issue-implement.yml](./issue-implement.yml) | [![Issue Implement](https://github.com/rysk-tanaka/csvr/actions/workflows/issue-implement.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/issue-implement.yml) | `claude-implement` ラベルで自動実装と PR 作成 | `issues` (labeled) |

## Claude 連携

| Workflow | Status | 主目的 | トリガー |
| --- | --- | --- | --- |
| [claude.yml](./claude.yml) | [![Claude Code](https://github.com/rysk-tanaka/csvr/actions/workflows/claude.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/claude.yml) | `@claude` メンションへの応答 | `issue_comment`, `pull_request_review_comment`, `issues`, `pull_request_review` |
| [claude-code-review.yml](./claude-code-review.yml) | [![Claude Code Review](https://github.com/rysk-tanaka/csvr/actions/workflows/claude-code-review.yml/badge.svg)](https://github.com/rysk-tanaka/csvr/actions/workflows/claude-code-review.yml) | `claude-review` ラベル付き PR の自動レビュー | `pull_request` (opened/synchronize/labeled/ready_for_review/reopened) |

## ワークフロー間の連携

| From | To | 連携条件 |
| --- | --- | --- |
| [issue-scan.yml](./issue-scan.yml) | [issue-implement.yml](./issue-implement.yml) | `claude-implement` ラベル付与で実装ワークフローを起動 |
| [issue-implement.yml](./issue-implement.yml) | [claude-code-review.yml](./claude-code-review.yml) | PR 作成後に `claude-review` ラベル付与でレビューを起動 |
| [auto-release.yml](./auto-release.yml) | release-on-version-change (external) | `workflow_call` でリリース処理を委譲 |

## claude-code-action の権限メモ

`anthropics/claude-code-action` を使うワークフローの `permissions` 設定に関する注意事項。

ソースコード上は bot/ユーザー作成 PR の区別なく常に OIDC 交換後の App トークンを使用する設計になっている。ただし、`issues: write` が不足していると `use_sticky_comment: true` のコメント投稿がサイレントに失敗することが確認されているため、必ず付与すること。

| 操作 | 必要な権限 |
| --- | --- |
| `use_sticky_comment: true`（PR へのサマリーコメント投稿） | `issues: write` |
| PR review / インラインコメント投稿 | `pull-requests: write` |
| コードの読み取り（checkout） | `contents: read` |
| ファイル編集・push | `contents: write` |
| OIDC トークン取得（必須） | `id-token: write` |
