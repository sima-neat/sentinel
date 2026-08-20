# ローカルエージェント API

Sentinelデーモンは、ローカルのUnixソケット`/run/simaai-sentinel/api.sock`を介して、バージョン管理されたHTTP/JSON APIを公開します。TCPポートでリッスンすることはありません。APIとターミナルUIは、同じキャッシュとロックされたチェックポイントストアを使用するため、エージェントによって開始されたトレースは、UIとCLIで即座に確認できます。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## エンドポイント

| 方法と経路 | 目的 |
| --- | --- |
| `GET /v1/health` | バージョン、最新性、サンプル数とメトリック数、エラー、およびアクティブなトレース。 |
| `GET /v1/cache` | ライブキャッシュドキュメントを完了します。 |
| `GET /v1/metrics` | メトリックの定義、単位、説明、および閾値。 |
| `GET /v1/samples/latest` | 最新のタイムスタンプ付きのメトリック値。 |
| `GET /v1/traces/active` | アクティブなトレース、または`null`。 |
| `POST /v1/traces` | 名前付きトレースを開始します。 |
| `POST /v1/traces/stop` | アクティブなトレースを停止し、続行します。 |
| `GET /v1/runs` | 実行中のタスクと完了したタスクの概要をリスト表示します。 |
| `GET /v1/runs/{name-or-id}` | 保存された実行結果と生データを取り出します。 |
| `GET /v1/compare?runs=A,B` | 2つ以上の実行結果を比較します。最初の実行結果を基準とします。 |

リクエストの開始例：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

アクティブなトレースを停止します。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

名前は一意である必要があり、アクティブにできるトレースは1つだけです。競合するライフサイクル操作は、HTTP 409を返します。不明な実行およびルートは、HTTP 404を返します。無効なリクエストは、HTTP 400を返します。利用できないメトリックの場合、レスポンスはJSON `null`を使用します。比較レスポンスには、デフォルトで実行メタデータ、統計、およびベースラインとの差分が含まれます。タイムスタンプ付きのサンプルが必要な場合にのみ、`raw=1`を追加してください。

## セキュリティと並行処理

このソケットは、DevKit 内でのみ使用され、Sentinel によってリモートからアクセスされることはありません。

これは、サポートされている制御操作がテレメトリの追跡を開始および停止するだけであり、API は実行を削除したり、ワークロードを実行したり、ハードウェアを変更したりしないため、意図的にローカルユーザーがアクセスできるように設計されています。リモートアクセスは、このソケットを転送したり、認証されていない TCP リスナーを追加したりするのではなく、認証された Kerrigan/Fleet Manager プロキシによって提供される必要があります。

キャッシュへの書き込みにはアトミックなリネームが使用されます。実行操作には、CLI、TUI、デーモンレコーダーと同じ排他的なファイルロックが使用されます。したがって、API を介して開始された追跡は、サポートされているインターフェースを介して安全に検査または停止できます。

## エージェントのスキル

インストール中、生成されたSentinelインストールスクリプトが、パッケージがビルドされたときの正確なSentinelGitコミットを使用して、`sima-cli
playbooks install`を実行します。これにより、ランタイムとスキルのバージョンが整合性を保ち、Vulcanアーティファクトにネストされたスキルリソースが追加されることはありません。プレイブックマネージャーは、サポートされている各エージェントに対してスキルをインストールし、そのソースコミットをローカルレジストリに記録します。

DevKitでのSentinelのインストールは、常に`sima-cli neat install
sentinel`から開始されるため、通常は個別のスキルインストール手順は必要ありません。DevKitパッケージがインストールされていない環境（たとえば、SDKコンテナーまたは開発者ワークステーション）でSentinelスキルを使用するには、その環境の`sima-cli`を使用して、GitHubから直接スキルをインストールします。

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

`main`に反映される前に、変更をテストするには、次のGitリファレンスを追加してください。

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

スキルの更新または削除は、引き続き`sima-cli playbooks`によって管理されます。
