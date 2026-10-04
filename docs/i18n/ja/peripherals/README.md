# 周辺機器

Sentinel は Modalix DevKit に接続されたデバイスのカタログを保持します。どのデバイスが存在するか、
どのように識別するか、何ができるかを記録します。Neat Core、Insight、sima-cli2、スクリプト、
エージェントはすべて同じカタログを読むため、ここで追加したデバイスタイプはすべての場所で同時に利用できます。

このカタログは拡張を前提に設計されています。最初のデバイスタイプはカメラですが、
別のタイプ（マイク、IMU、LiDAR など）を追加しても、カタログ、API、CLI、クライアントを変更する必要はありません。

## 動作の仕組み

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

**プロバイダー**は Sentinel 内の Rust コードで、カーネルインターフェースから 1 種類の
デバイス群を検出してレコードを返します。それ以外の処理、つまりホットプラグによる起床、
デバウンス、障害の分離、失敗したプロバイダーの最終正常レコードの保持、安定したリビジョン、
変更ログ、API、CLI は Sentinel が担当します。

検出処理は低コストになるよう設計されています。変更がなければスレッドは CPU を使用せず、
デーモンより nice 値を 10 段階低くして実行し、イベントと更新要求のバーストを 1 回のスキャンにまとめます。

## ページ

| ページ | 対象 |
| --- | --- |
| [デバイスタイプの追加](adding-a-device-type.md) | 新しい種類のデバイス対応を追加するコントリビューター |
| [デバイスタイプ](device-types/README.md) | サポート対象タイプごとのレコード形式。[カメラ](device-types/camera.md)から開始 |
| [ローカルエージェント API](../api.md) | `GET /v1/peripherals`、`POST /v1/peripherals/refresh`、`since_revision` によるポーリング |

## カタログの使用

```bash
simaai-sentinel peripherals            # table
simaai-sentinel peripherals --json     # the catalog document
simaai-sentinel peripherals --refresh  # rescan first
curl --unix-socket /run/simaai-sentinel/api.sock http://localhost/v1/peripherals
```

Neat アプリケーションでは、`simaai::neat::peripherals::list()`（C++）と
`pyneat.peripherals.list()`（Python）が同じカタログを返します。各デバイスは詳細を JSON
（`details_json` / `details`）として持つため、Core が型付きフィールドを追加する前でも、
Neat アプリケーションから新しいデバイスタイプを利用できます。

<a id="support-rules"></a>

## サポートルール

各カメラモードには `supported` と `reason` があり、インストール済み Neat Core の
`CameraInput` がそのモードを受け入れるかを示します。これは Sentinel ではなく Neat Core が決定します。
Neat Core はルールを `/usr/share/simaai-sentinel/support/neat-core.json` にインストールし、
周辺機器スレッドはカタログを比較して書き込む前に各モードへ適用します。そのため Core の更新も
他の変更と同様に `revision` を進めます。Sentinel はディレクトリを監視し、ハードウェアを
再スキャンせずにルールを再適用します。Neat Core がない場合、すべてのモードは
`supported: false` となり、理由には Core が未インストールであることが示されます。

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

ルールはこの順番で検査され、最初の失敗がモードの `reason` になります。サイズ範囲は
サポート済みとは扱われません。`isp_output` がある場合、そのモードは ISP 出力サイズである必要があります。
カタログ最上位の `support` は、`state`（`applied`、`not_installed`、`invalid`、または
無効な更新後も以前のルールを使う `stale`）、`source`、`path` を報告します。

Sentinel は `/usr/share/simaai-sentinel/support/` を作成しますが、そこへファイルを
インストールしません。したがって Sentinel と Neat Core が同じパスを所有せず、個別に
インストール、更新、削除できます。現時点では形式 1 のみです。形式が追加されても Sentinel は
古い形式を読み続けます。Sentinel が知らない新しい形式のルールファイルでは以前のルールを維持し、
Sentinel の更新が必要であることを報告します。

## デーモンオプション

| オプション | デフォルト | 意味 |
| --- | --- | --- |
| `--peripherals-file PATH` | `/run/simaai-sentinel/peripherals.json` | カタログを書き込み、`simaai-sentinel peripherals` が読み取る場所 |
| `--support-rules PATH` | `/usr/share/simaai-sentinel/support/neat-core.json` | Neat Core のカメラサポートルール |
| `--no-peripherals` | off | 周辺機器検出なしでデーモンを実行 |
