# ローカルエージェント API

Sentinelデーモンは、ローカルのUnixソケット`/run/simaai-sentinel/api.sock`を介して、バージョン管理されたHTTP/JSON APIを公開します。TCPポートでリッスンすることはありません。APIとターミナルUIは、同じキャッシュとロックされたチェックポイントストアを使用するため、エージェントによって開始されたトレースは、UIとCLIで即座に確認できます。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## エンドポイント

| 方法と経路 | 目的 |
| --- | --- |
| `GET /v1/health` | バージョン、最新性、サンプル数とメトリック数、エラー、アクティブなトレース、および `peripherals` の概要。 |
| `GET /v1/cache` | ライブキャッシュドキュメントを完了します。 |
| `GET /v1/metrics` | メトリックの定義、単位、説明、および閾値。 |
| `GET /v1/samples/latest` | 最新のタイムスタンプ付きのメトリック値。 |
| `GET /v1/traces/active` | アクティブなトレース、または`null`。 |
| `POST /v1/traces` | 名前付きトレースを開始します。 |
| `POST /v1/traces/stop` | アクティブなトレースを停止し、続行します。 |
| `GET /v1/runs` | 実行中のタスクと完了したタスクの概要をリスト表示します。 |
| `GET /v1/runs/{name-or-id}` | 保存された実行結果と生データを取り出します。 |
| `GET /v1/compare?runs=A,B` | 2つ以上の実行結果を比較します。最初の実行結果を基準とします。 |
| `GET /v1/peripherals` | 現在の周辺機器カタログ。変更がない場合に短い `unchanged` 応答を得るには `since_revision=N` を追加します。 |
| `POST /v1/peripherals/refresh` | 再スキャンを要求し、`target_scan_sequence` を返します。 |

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

## 周辺機器カタログ

`GET /v1/peripherals` は、周辺機器スレッドが
`/run/simaai-sentinel/peripherals.json` に書き込んだ接続済みデバイスの
カタログを、そのまま返します。検出の仕組み、各デバイスタイプのレコード形式、
およびタイプの追加方法は[周辺機器](peripherals/README.md)を参照してください。

| フィールド | 意味 |
| --- | --- |
| `instance_id` | デーモン起動ごとに新しくなります。値が異なる場合はデーモンが再起動しています。 |
| `state`, `ready` | 最初のスキャンまでは `starting`、その後は `ready`。プロバイダー、イベントモニター、またはサポートルールに問題がある間は `degraded`。停止したデーモンはエラー `peripherals.stopped` とともに `degraded` を残します。 |
| `revision` | デバイス、問題、エラー、サポートルールの状態など、クライアントに見える内容が変わるたびに増加します。 |
| `scan_sequence` | 変更がない場合も含め、完了したスキャンごとに増加します。 |
| `stale`, `issues`, `error` | 失敗したプロバイダーは、`retained_last_good` と記された最後の正常なレコードを保持します。他のプロバイダーには影響しません。`error` はホットプラグ監視が利用できない場合など、デーモン自体の問題を説明します。 |
| `changes` | 直近256件の変更：`added`、`removed`、`changed`、`error`、`recovered`。それぞれに `sequence` と `revision` があります。 |
| `support` | カメラモードを分類した Neat Core ルール。[サポートルール](peripherals/README.md#support-rules)を参照してください。 |
| `devices` | `{id, type, provider, <type>: {...}}`。`id` は同じポートへの再接続後も変わらず、`/dev/videoN` 名ではありません。 |

低コストでポーリングするには、最後に確認した `revision` と `instance_id` を
送ります：`GET /v1/peripherals?since_revision=7&instance_id=<id>`。クライアント
に見える変更がなければ、応答は
`{"unchanged": true, "revision": 7, "scan_sequence": ..., "instance_id": ...}`、
それ以外は完全なカタログです。リビジョンはデーモンごとに再開するため、
`instance_id` のない `since_revision` は HTTP 400 で拒否されます。

`POST /v1/peripherals/refresh` はスキャンを予約し、
`{"accepted": true, "target_scan_sequence": N, "instance_id": ...}` を返します。
同じ `instance_id` のカタログが `scan_sequence` `N` に達すると更新完了です。
同時に届いた要求は1回のスキャンを共有します。HTTP 503 は周辺機器の検出が
実行されていないか停止した、またはカタログファイルを書き込めず（例：`/run`
が満杯）古くなっていることを意味します。`error` に理由が入り、詳細は
ジャーナルにあります。この場合 `/v1/health` は `"peripherals": null` を
報告します。Sentinel は失敗した書き込みを毎秒再試行し、成功するとカタログの
提供を再開します。`GET /v1/peripherals` の HTTP 503 はカタログを読み取れない
場合もあります。

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
