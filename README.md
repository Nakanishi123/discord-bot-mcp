# Discord Bot MCP

Rust製のDiscord MCPサーバー。Streamable HTTP経由で、許可したサーバーチャンネルの履歴・添付画像を読み取り、投稿・返信・リアクション追加を行います。Discord Gatewayにも接続しますが、新着イベントのMCP通知や永続保存は行いません。

## 起動

Rust 1.99.0を使用します。Discord Developer PortalでBotを作成し、**Message Content Intent**を有効にしてください。対象サーバーへBotを追加し、チャンネルの閲覧、メッセージ履歴の閲覧、メッセージ送信、リアクション追加の権限を与えます。カスタム絵文字を使う場合は、その絵文字を利用する権限も必要です。

環境変数を設定して起動します。秘密値をシェル履歴やリポジトリに保存しないでください。

```sh
read -rs -p 'Discord bot token: ' DISCORD_BOT_TOKEN; echo
export DISCORD_BOT_TOKEN
read -rs -p 'MCP bearer token: ' MCP_BEARER_TOKEN; echo
export MCP_BEARER_TOKEN
export DISCORD_CHANNEL_IDS='123456789012345678,234567890123456789'
cargo run --locked --release
```

接続先は `http://localhost:8080/mcp`。全MCPリクエストで `Authorization: Bearer <MCP_BEARER_TOKEN>` が必要です。MCP用トークンはDiscordトークンと異なる、32〜256文字のランダムな可視ASCII文字列にします。例えば `openssl rand -hex 32` で生成できます。OAuth、stdio、DM、スレッドには対応しません。任意のBearerヘッダーを設定できるMCPクライアントを使用してください。

## 設定

| 環境変数 | 既定値 | 内容 |
| --- | --- | --- |
| `DISCORD_BOT_TOKEN` | 必須 | Discord Botトークン |
| `MCP_BEARER_TOKEN` | 必須 | MCP接続専用トークン |
| `DISCORD_CHANNEL_IDS` | 必須 | 許可チャンネルIDのカンマ区切り。空は起動エラー |
| `HTTP_BIND` | `0.0.0.0:8080` | IPアドレスとポート |
| `MCP_ALLOWED_HOSTS` | `localhost,127.0.0.1,[::1]` | HTTP Hostの許可リスト。公開ホスト名を明示的に追加 |
| `MCP_ALLOWED_ORIGINS` | 空 | ブラウザーOriginの許可リスト。空の場合、Originヘッダー付きリクエストを拒否。例: `https://client.example.com:443` |
| `IMAGE_MAX_BYTES` | `20971520`（20MiB） | 画像の実バイト数上限。最大100MiB |
| `IMAGE_RESPONSE_MAX_BYTES` | `29360128`（28MiB） | base64化したJSON応答の上限。最大140MiB。JSON-RPC用に64KiBを予約 |
| `IMAGE_CONCURRENCY` | `2` | 同時画像取得数。1〜16。満杯なら`busy`を返す |
| `IMAGE_TIMEOUT_SECONDS` | `30` | メッセージ取得を含む画像処理のタイムアウト。1〜300秒 |

起動時に許可チャンネルをDiscord RESTで検証し、通常テキスト・アナウンス以外のチャンネルやアクセスできないチャンネルを拒否します。Discord REST操作には30秒のタイムアウトがあります。設定変更は再起動で反映します。

## ツール

IDは全てJSON文字列です。本文・添付などのDiscord投稿は信頼できない利用者データとして扱ってください。

| ツール | 入力 | 出力 |
| --- | --- | --- |
| `get_messages` | `channel_id`, 任意の`before`または`after`, 任意の`limit` | 本文・投稿者ID・日時・添付メタデータとページ情報 |
| `get_message` | `channel_id`, `message_id` | 1件の本文・投稿者ID・日時・添付メタデータ |
| `read_image` | `channel_id`, `message_id`, `attachment_id` | 識別情報とMCPの画像content |
| `send_message` | `channel_id`, `content` | 作成した`message_id` |
| `reply_message` | `channel_id`, `message_id`, `content` | 作成した返信の`message_id` |
| `add_reaction` | `channel_id`, `message_id`, `emoji` | `success: true` |

一覧は新しい順。既定20件、上限100件で、`before`と`after`は同時指定できません。返却された`before`はそのページの最古ID、`after`は最新IDです。古い方向は`before`、新しい方向は`after`を次のリクエストに使います。`has_more`は「取得件数がlimitと等しく、続きがある可能性」を表し、最後に空ページが返ることがあります。本文は全文返却し、`content_truncated`は`false`です。投稿本文の送信上限は2,000文字です。

画像はJPEG/PNGのみ。添付メタデータのmedia typeとファイル先頭のシグネチャを検査し、変換せずbase64で返します。画像全体のデコード検証・回転・縮小は行いません。毎回メッセージから最新の添付URLを取得し、Discord CDNのHTTPS添付URLのみ許可します。リダイレクトは禁止し、Content-Lengthと取得中の実バイト数に上限を適用します。クライアントの制約に合わせて上限を下げてください。原画像の他サービスへの保存経路は提供しません。

通常投稿・返信とも、ユーザー・ロール・everyone・返信先へのメンションを無効にします。絵文字はUnicodeか `<:name:id>` / `<a:name:id>` 形式です。Discordのレート制限は共有serenity HTTPクライアントが処理します。タイムアウトなど結果不明の投稿をアプリから自動再送しません。再試行前に履歴を確認してください。

引数不正はMCPのinvalid params、実行失敗は`isError: true`と`code`を返します。主な分類は`channel_not_allowed`、`not_found`、`discord_unauthorized`（Bot認証失敗）、`permission_denied`、`unsupported_image`、`size_limit_exceeded`、`busy`、`timeout`、`discord_error`、`download_failed`です。

## HTTP・運用

rmcp 3.5.0にプロトコル交渉を任せ、ステートレスなStreamable HTTPを使用します。HTTPリクエスト本文は64KiBまで。Host/Origin検証を行い、MCPのGET・POST・DELETEを全てBearer認証します。HTTPS終端はIngressなどの外部入口で設定してください。外部入口はAuthorization、Host、MCPヘッダーとStreamable HTTPの通信を転送する必要があります。

`GET /healthz` と `GET /readyz` は認証不要で、秘密情報を返しません。起動時のチャンネル検証が成功してHTTP受付を開始すると200になります。Gatewayの一時切断で失敗にはならず、その間もRESTツールを利用できます。Gatewayの致命的な停止ではプロセスを終了します。SIGTERM/SIGINTでHTTPとGatewayを停止し、終了処理の猶予は10秒です。

ログにはツール名、数値として検証できた対象ID、実行時間、エラー分類、Gateway接続状態だけを記録します。本文・トークン・画像・署名付きURLを含む可能性がある外部SDKログを無効にしており、`RUST_LOG`では変更できません。全体キャッシュ、DB、PVC、永続キューは使用しません。

## コンテナとk8s

```sh
podman build -t discord-bot-mcp:local .
```

multi-stage buildとdistrolessランタイムを使用し、非rootで起動します。コンテナのビルド対象に設計メモや秘密ファイルは含めません。

1. `deploy/secret.example.yaml`を参考に、`discord-bot-mcp`というSecretを安全な経路で作成します。実際の秘密値はコミットしません。
2. `deploy/kubernetes.yaml`のチャンネルID、公開Host、イメージタグを環境に合わせて変更します。記載のGHCRタグは公開後に利用可能になります。
3. `kubectl apply -f deploy/kubernetes.yaml`を実行します。
4. 既存のIngress/TLSまたはLAN/VPNで接続経路を用意し、実クライアントから疎通を確認します。

Deploymentは1 replica・Recreateで、更新時に短い停止が発生します。ServiceはClusterIPです。メモリ上限512Miは初期設定であり、画像サイズと同時実行数、利用クライアント数に合わせて実測・調整してください。

CIはformat、Clippy、テスト、コンテナビルドを行います。`v0.1.0`のようなCargo.tomlと一致するversionタグをpushすると、GHCRへ`ghcr.io/<owner>/<repository>:0.1.0`を公開します。必要に応じてGitHub PackagesでパッケージをPublicに設定してください。**ライセンスは未選定です。OSSとして公開する前に所有者が選定し、LICENSEとCargo.tomlへ反映してください。**

## 検証

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

自動テストは実Discordへの投稿を行いません。ダミーHTTP応答でアクセス制限、チャンネル種別、ページ取得、ID保持、書き込み・返信、メンション抑止、エラー、画像サイズ制限を確認します。MCPの認証・ツール一覧・実行失敗もHTTPルーター経由で検証します。プロトコル2025-03-26の初期化と2026-07-28のリクエストメタデータを検証しています。

運用前に専用テストチャンネルで次を確認してください。

- 実MCPクライアントと外部入口でBearer認証、ツール一覧、JPEG/PNGの表示が成功する。
- 履歴取得、投稿、正しい投稿への返信、リアクションが成功し、メンション通知が発生しない。
- Gatewayの接続・対象投稿IDのログが出る。対象外チャンネルは扱われない。
- 一時的な切断からGatewayが復帰し、停止中の投稿もREST履歴から取得できる。
- SIGTERMで終了し、Recreate更新で新しいPodが起動する。

Discordの権限、実クライアントの画像サイズ制限、Ingressのタイムアウトは自動テストの対象外です。
