# デバイスタイプの追加

このページは、Sentinel に新しい種類のデバイスを検出させるコントリビューター向けです。
カーネルインターフェースの確認、プロバイダーの作成とテスト、アプリケーション開発者が
利用できるようにするための文書化について説明します。

## 1. カーネルがデバイスを記述していることを確認する

各プロバイダーは Sentinel 内の Rust コードで、sysfs、`/proc`、uevent、読み取り専用 ioctl
（V4L2、ALSA、IIO、media controller など）のカーネルインターフェースを読み取ります。
Sentinel はデバイス検出のために別プログラムを実行したり、libcamera や GStreamer のような
ユーザー空間スタックをロードしたりしません。実装前に、アプリケーションが必要とする情報を
カーネルが公開していることを確認してください。

カメラプロバイダーが実例です。USB カメラは `src/peripherals/v4l2/`、MIPI カメラは
`src/peripherals/mipi/` にあります。

## 2. すべてのプロバイダーが従うルール

1. **読み取り専用。** デバイスを照会するだけで、設定、ストリーミング、所有、カーネル状態の
   変更をしてはいけません。デバイスノードは `O_RDONLY | O_NONBLOCK` で開きます。
   実行中のアプリケーションが検出処理に気付いてはいけません。
2. **安定した識別子。** レコードの `id` は、再接続、再起動、番号変更後も同じでなければなりません。
   バストポロジー、シリアル、カーネルエンティティ名などの安定した属性から作り、
   `/dev/videoN`、カード番号、列挙順序は使いません。`camera:...`、`microphone:...` のように
   タイプを接頭辞にします。
3. **手元の個体ではなくクラス全体を対象にする。** 手元のデバイスは最初のテストフィクスチャであり、
   仕様そのものではありません。実装前に、同じクラスの他のデバイスがどう異なるか（個数、形式、
   範囲と離散値、欠ける可能性がある任意フィールド、複合デバイス、複数の同一デバイス、使用中に
   変わる値）を列挙し、それぞれを処理してください。
4. **失敗ではなく縮退。** 任意フィールドがなければ省略します。任意部分が読めなければ理由を
   レコードに示します。正しい一覧を生成できない場合だけスキャンを失敗させます。その場合、
   Sentinel は最後の正常なレコードを保持し、エラーを表示します。
5. **事実のみ。** デバイスとカーネルが示す内容を報告します。Neat コンポーネントがデバイスを
   サポートするかどうかは、プロバイダーではなく、そのコンポーネントのサポートルールが決定します。
6. **処理量を制限する。** プロバイダーはデーモン内で実行され、強制終了できないため、ブロックしては
   なりません。壊れたデバイスや悪意あるデバイスに無限の処理をさせてもいけません。各列挙ループは
   1024 エントリ（`MAX_ENUMERATION_ENTRIES`）、デバイスごとの 1 スキャンは 4096 クエリ
   （`EnumerationBudget`、`src/peripherals/videodev2.rs` の
   `MAX_DEVICE_ENUMERATIONS`）に制限します。

## 3. レコード

プロバイダーはレコードの一覧を返します。

```json
{"id": "microphone:usb-1-2.3:1.2", "type": "microphone",
 "provider": "daemon.audio.alsa", "details": {"channels": 2, "formats": ["S16_LE"]}}
```

| フィールド | ルール |
| --- | --- |
| `id` | 空でなく、全プロバイダー間で一意で、安定していること（上記参照） |
| `type` | 小文字、数字、`_`、`-` のみで文字から始まり、最大 64 文字。`id`、`type`、`provider` は不可 |
| `provider` | プロバイダー名。例: `daemon.audio.alsa` |
| `details` | JSON オブジェクト。各フィールドはタイプのスキーマで、[デバイスタイプ](device-types/README.md)に記載 |

カタログでは、`details` はタイプ名のキー配下に公開されます。
`{"id", "type", "provider", "microphone": {...}}`。クライアントは未知のタイプを JSON として
読むため、API、CLI、Insight、Neat Core の `details` に変更を加えなくても新しいタイプが現れます。

## 4. プロバイダーの作成

1. `src/peripherals/<name>/`（または `<name>.rs`）を作り、
   `src/peripherals/model.rs` の `Provider` トレイトを実装します。

   ```rust
   impl Provider for MicrophoneProvider {
       fn name(&self) -> &str { "daemon.audio.alsa" }
       // Kernel uevent subsystems that should trigger a rescan.
       fn subsystems(&self) -> &[String] { &self.subsystems } // ["sound"]
       fn discover(&mut self) -> Result<Vec<Record>, ProviderError> { ... }
   }
   ```

2. `src/peripherals/mod.rs` の `builtin_providers()` に 1 行追加して登録します。
3. ファイルシステムのルートを注入可能にし（例: `with_roots(sys, dev)`）、ioctl 呼び出しを
   小さなトレイトの背後に置いて、ハードウェアなしでテストできるようにします。カメラプロバイダーが例です。
4. 失敗を `ProviderError` コード `io.permission_denied`、`io.open`、
   `peripherals.discovery_failed` に対応付けます。

## 5. テスト

Sentinel と同じ方法でプロバイダーを 1 回実行し、カタログに追加する内容を確認します。

```bash
simaai-sentinel peripherals --test-provider daemon.audio.alsa
```

このコマンドはデーモンと同じプロバイダー単位の検査でレコードを検証し、サポートルールを適用して
結果を表示します。失敗時は 0 以外で終了します。ID はプロバイダー間でも一意である必要があるため、
タイプとプロバイダー固有のキーを接頭辞にしてください。

ユニットテストは必須です。

- ルール 3 で列挙した変動軸ごとに 1 テスト。
- 実機キャプチャがある場合はそこから転記したフィクスチャを使い、それ以外は形式に忠実な
  合成フィクスチャを使い、どちらかを明示する。
- 権限拒否、スキャン中に消えるデバイス、不正な応答などのエラーパス。

プルリクエスト前に、リポジトリの CI 手順をローカルで実行します。
`cargo fmt --check`、`cargo check --locked`、`cargo test --locked`、
`scripts/build_vulcan_package.sh`。

## 6. タイプの文書化

[テンプレート](device-types/TEMPLATE.md)から `docs/peripherals/device-types/<type>.md` を追加し、
[デバイスタイプ索引](device-types/README.md)に掲載します。アプリケーション開発者はこのページを
参照してレコードを読みます。実機で検証した動作とフィクスチャのみで検証した動作を明記してください。

## チェックリスト

- [ ] アプリケーションが必要とする情報をカーネルが公開している
- [ ] 読み取り専用で、設定、ストリーミング、所有を行わない
- [ ] 安定した属性から作り、タイプを接頭辞にした安定した `id`
- [ ] 変動軸を列挙し、それぞれをテストした
- [ ] 任意データの欠落時は失敗ではなく縮退する
- [ ] `--test-provider` の出力を確認した
- [ ] デバイスタイプページを追加し、索引に掲載した
- [ ] リポジトリの CI 手順がローカルで成功した
