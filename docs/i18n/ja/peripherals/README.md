# 周辺機器

Sentinel は、Modalix DevKit に接続されているデバイスのカタログを保持します：どのデバイスが存在するか、それらをどのように識別するか、そしてそれらが何をできるか。Neat Core、Insight、sima-cli2、スクリプトおよびエージェントはすべて同じカタログを参照するため、ここに追加されたデバイスタイプは一度にすべての場所で見えるようになります。

このカタログは拡張性のために作られています。カメラやマイクは組み込みのデバイスタイプです；別のタイプ（IMU、LiDARなど）を追加しても、カタログ、API、CLI、またはクライアントを変更する必要はありません。

## 仕組み

```text
kernel hot-plug event ─┐
refresh request ───────┤
                       ▼
            peripherals thread (one per daemon, sleeps until woken)
              1. run every provider (read-only)
              2. apply Neat Core's support rules (cameras)
              3. compare with the last catalog; revision +1 if changed
              4. write /run/simaai-sentinel/peripherals.json
                       │
                       ▼
            GET /v1/peripherals  ·  simaai-sentinel peripherals
```

**プロバイダー**とは、カーネルインターフェースから1つのデバイスファミリーを発見し、レコードを返すSentinel内のRustコードです。Sentinelはそれ以外のすべてを行います：ホットプラグ時の起動、デバウンス、障害の分離、障害発生プロバイダーの最後の正常レコードの保持、安定したリビジョン、変更ログ、APIおよびCLIです。

発見（ディスカバリ）は設計上軽量です：スレッドは何も変化がない間はCPUを使用せず、デーモンより10段階低いniceレベルで動作し、イベントのバーストや更新要求を1回のスキャンにまとめます。

## ページ

| ページ | 対象 |
| --- | --- |
| [デバイスタイプを追加する](adding-a-device-type.md) | 新しい種類のデバイスのサポートを追加するコントリビューター |
| [デバイスの種類](device-types/README.md) | サポートされている各タイプのレコード形式: [カメラ](device-types/camera.md), [マイクロフォン](device-types/microphone.md) |
| [ローカルエージェントAPI](../api.md) | `GET /v1/peripherals`, `POST /v1/peripherals/refresh`、投票で `since_revision` |

## カタログを使う

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

Neatアプリケーションから、`simaai::neat::peripherals::list()`（C++）および `pyneat.peripherals.list()`（Python）は同じカタログを返します。すべてのデバイスは JSON（`details_json` / `details`）としてその詳細を保持しているため、新しいデバイスタイプは、Coreが型付きフィールドを追加する前でもNeatアプリケーションから使用可能です。

<a id="support-rules"></a>

## サポート規則

各カメラモードには `supported` と `reason` が付随しています：インストールされている Neat Core の `CameraInput` がそれを受け入れるかどうかです。Sentinel はこれを決定しません。Neat Core は自分のルールを `/usr/share/simaai-sentinel/support/neat-core.json` にインストールし、周辺機器スレッドはカタログが比較され書き込まれる前にすべてのモードにそのルールを適用するので、Core のアップグレードは他の変更と同様に `revision` を増加させます。Sentinel はディレクトリを監視し、ハードウェアを再スキャンせずにルールを再適用します。Neat Core がない場合、すべてのモードは `supported: false` になり、理由として Core がインストールされていないことが示されます。

```json
{
  "format": 1,
  "source": "neat-core 0.4.0",
  "camera": {
    "backends": {"accept": ["mipi"], "reason": "..."},
    "formats": {"accept": ["NV12"], "reason": "..."},
    "framerates": {"accept": [{"num": 30, "den": 1}], "reason": "..."},
    "isp_output": {"reason": "..."}
  }
}
```

ルールはその順序でチェックされ、最初の失敗がモードの`reason`となります。サイズ範囲はサポート済みとしてマークされることはありません。`isp_output`がある場合、モードはISP出力サイズである必要があります。カタログのトップレベル`support`は`state`（`applied`、`not_installed`、`invalid`、または無効な更新により以前のルールが使用された場合の`stale`）、`source`および`path`を報告します。

Sentinelは`/usr/share/simaai-sentinel/support/`を作成しますが、その中にファイルをインストールすることはないため、SentinelとNeat Coreは同じパスを主張せず、それぞれ独立してインストール、アップグレード、アンインストールを行います。現在のところフォーマットは1のみで、フォーマットが追加されても、Sentinelは古いフォーマットを読み続けます。Sentinelより新しいフォーマットのルールファイルは、以前のルールを引き続き使用し、Sentinelの更新が必要であることを報告します。

## デーモンオプション

| オプション | デフォルト | 意味 |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | カタログが書き込まれ、`simaai-sentinel peripherals` によって読み取られる場所 |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Neat Coreのカメラサポート規則 |
| `--no-peripherals` | オフ | 周辺機器の検出なしでデーモンを実行 |
