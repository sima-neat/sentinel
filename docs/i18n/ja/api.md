# ローカルエージェント API

Sentinelデーモンは、ローカルのUnixソケット`/run/simaai-sentinel/api.sock`を介して、バージョン管理されたHTTP/JSON APIを公開します。
TCPポートでリッスンすることはありません。
APIとターミナルUIは同じキャッシュとロックされたチェックポイントストアを使用するため、
エージェントが開始したトレースはUIとCLIですぐに確認できます。

```bash
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/health
```

## エンドポイント

| メソッドとパス | 目的 |
| --- | --- |
| `GET /v1/health` | バージョン、データの鮮度、サンプル数とメトリック数、エラー、およびアクティブなトレース。 |
| `GET /v1/cache` | ライブキャッシュドキュメント全体。 |
| `GET /v1/metrics` | メトリックの定義、単位、説明、および閾値。 |
| `GET /v1/samples/latest` | タイムスタンプ付きの最新のメトリック値。 |
| `GET /v1/traces/active` | アクティブなトレース、または`null`。 |
| `POST /v1/traces` | 名前付きトレースを開始します。 |
| `POST /v1/traces/stop` | アクティブなトレースを停止して保存します。 |
| `POST /v1/traces/{id}/stop` | アクティブなトレースのIDがまだ一致する場合にのみ、そのトレースを停止します。 |
| `GET /v1/runs` | アクティブな実行と完了した実行の概要を一覧表示します。 |
| `GET /v1/runs/{name-or-id}` | 保存された実行とその生サンプルを取得します。 |
| `GET /v1/compare?runs=A,B` | 2つ以上の実行を比較します。最初の実行がベースラインになります。 |
| `GET /v1/peripherals` | 接続されている周辺機器をメモリから返します。 |
| `POST /v1/peripherals/refresh` | 再スキャンしてから、新しいカタログを返します。 |

開始リクエストの例：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -H 'Content-Type: application/json' \
  -d '{"name":"baseline","note":"before optimization","tags":["compiler-v1"]}' \
  http://localhost/v1/traces
```

アクティブなトレースを停止します：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/stop
```

先にアクティブなトレースを読み取ったクライアントは、停止するときにそのIDを含めてください：

```bash
curl --unix-socket /run/simaai-sentinel/api.sock \
  -X POST http://localhost/v1/traces/20261004T120000.000Z-baseline/stop
```

別のクライアントがトレースを置き換えた場合、条件付きの形式は
何も停止せずにHTTP 409を返します。IDの比較と停止は、同じ
実行ストアのロックの下で行われます。

名前は一意である必要があり、アクティブにできるトレースは1つだけです。
競合するライフサイクル操作はHTTP 409を返します。不明な実行とルートはHTTP 404を返します。
無効なリクエストはHTTP 400を返します。利用できないメトリックについて、レスポンスはJSON `null`を使用します。
比較レスポンスには、デフォルトで実行メタデータ、統計、およびベースラインとの差分が含まれます。
タイムスタンプ付きのサンプルが必要な場合にのみ、`raw=1`を追加してください。

## 周辺機器

検出スレッドは、デーモンの起動時、カーネルがデバイスの変更を通知したとき、
およびリフレッシュ要求があったときに、ボードの周辺機器をスキャンします。
カーネルインターフェースを読み取るだけで、ストリームを開くことはありません。
`GET /v1/peripherals`は最新の結果を返します：

```json
{"revision": 1791155282460, "observed_at": "2026-10-04T23:08:02.460Z",
 "devices": [{"type": "camera", "id": "camera:v4l2:3f2a9c0d41b7e650", ...}],
 "errors": [{"provider": "camera.v4l2", "code": "io.permission_denied", "reason": "..."}]}
```

| フィールド | 意味 |
| --- | --- |
| `revision` | `devices`または`errors`が変わるたびに変化します。デーモンの起動時にランダムな値から始まるため、再起動や時刻の変更によって同じ値が繰り返される可能性は非常に低くなっています。比較は等価性の判定にのみ使用してください。値は常に2^52未満であるため、倍精度浮動小数点数を使用するJSONリーダーでも正確に表現されます。 |
| `observed_at` | この結果の元になったスキャンの開始時刻。最初のスキャンが完了するまでは`null`です。 |
| `devices` | デバイスごとに1つのオブジェクトで、`type`でタグ付けされます。`id`は同じポートへの再接続後も変わらず、`/dev/videoN`の名前になることはありません。 |
| `errors` | 最新のスキャンで失敗したプロバイダー。失敗したプロバイダーのデバイスは、最後に成功したスキャンの結果が`devices`に残ります。`hotplug.unavailable`は、カーネルのueventを受信できないため、再スキャンがリフレッシュ時にしか行われないことを意味します。 |

Sentinelはハードウェアの事実のみを報告します。
アプリケーションがデバイスやモードをサポートするかどうかは、そのアプリケーションが判断します。
`CameraInput`の場合は、Neat Coreがこれを判断します。

`POST /v1/peripherals/refresh`は、リクエスト後に開始されるスキャンを待ち、
そのカタログをHTTP 200で返します。これは`GET`と同じドキュメントです。
同時に行われたリフレッシュはスキャンを共有します。スキャンが10秒以内に完了しない場合はHTTP 504を返し、
すでに8件のリフレッシュ要求が待機している場合はHTTP 429を返します。
検出が無効化されている場合（`--no-peripherals`）または停止している場合、
どちらのルートもHTTP 503を返します。

`simaai-sentinel peripherals`は同じ結果を表形式で出力します。生のドキュメントを得るには`--json`を、
先に再スキャンするには`--refresh`を追加してください。

デバイスレコードはタイプごとに説明しています：[USBカメラ](peripherals/camera.md)、
[MIPI CSI-2カメラ](peripherals/camera-mipi.md)、
[マイク](peripherals/microphone.md)。

[`peripherals/catalog-example.json`](peripherals/catalog-example.json)は、
DevKitから取得した完全なレスポンスです（USBカメラはフォーマットごとに1つのモードに絞っています）。
Sentinelのテストがこれをスキーマに照らして検証しているため、
クライアントはこれを基準にテストできます。

## セキュリティと並行処理

このソケットはDevKit上のローカルなものであり、Sentinelがリモートに公開することはありません。
サポートされている制御操作はテレメトリトレースの開始と停止のみであり、
APIは実行の削除、ワークロードの実行、ハードウェアの変更を行わないため、
意図的にローカルユーザーがアクセスできるようにしています。
リモートアクセスは、このソケットを転送したり認証なしのTCPリスナーを追加したりするのではなく、
認証されたKerrigan/Fleet Managerプロキシによって提供する必要があります。

キャッシュの書き込みにはアトミックなリネームを使用します。
実行操作には、CLI、TUI、デーモンレコーダーと同じ排他的ファイルロックを使用します。
IDを含む停止ルートは、クライアントが観測したトレースを置き換えたトレースを停止してはならない場合に使用してください。

## エージェントスキル

インストール中、生成されたSentinelインストールスクリプトは、
パッケージのビルド元である正確なSentinel Gitコミットを指定して`sima-cli
playbooks install`を実行します。
これにより、Vulcanアーティファクトにネストされたスキルリソースを追加することなく、ランタイムとスキルのリビジョンが揃います。
プレイブックマネージャーは、サポートされている各エージェント向けにスキルをインストールし、
そのソースコミットをローカルレジストリに記録します。

DevKitへのSentinelのインストールは常に`sima-cli neat install
sentinel`から始まるため、通常、そこでは別途スキルをインストールする手順は必要ありません。
DevKitパッケージがインストールされていない環境（たとえばSDKコンテナーや開発者ワークステーション）で
Sentinelスキルを使用するには、
その環境の`sima-cli`を使用して、GitHubから直接スキルをインストールします：

```bash
sima-cli playbooks install gh:sima-neat/sentinel/skills/use-sentinel
```

`main`に反映される前のリビジョンをテストするには、Git refを末尾に付加します：

```bash
sima-cli playbooks install --force \
  gh:sima-neat/sentinel/skills/use-sentinel@feature/checkpoint-run-comparison
```

スキルの更新や削除は、引き続き`sima-cli playbooks`で管理します。
