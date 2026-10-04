# <Type name>

<!-- One paragraph: what this type covers and which devices are in its class. -->

- **タイプトークン:** `<type>`
- **プロバイダー:** `<provider name>`
- **再スキャントリガー:** `<uevent subsystems>`

## 識別子

`id` をどの安定した属性から作り、再接続、再起動、番号変更後も何によって安定性を保つかを記述します。
例: `<type>:<stable key>`。

## 詳細

| フィールド | 型 | 常に存在 | 意味 | 情報源 |
| --- | --- | --- | --- | --- |
| `<field>` | string | yes | <what it means> | <sysfs file, ioctl, vendor API> |

## レコード例

```json
{"id": "<type>:...", "type": "<type>", "provider": "...", "<type>": {}}
```

## 対応する変動

このクラスのデバイスがどのように異なり、それぞれをどう処理するかを列挙します
（個数、形式、範囲、任意フィールド、複合デバイス、複数の同一デバイス、使用中に変わる値）。

## サポートルール

Neat コンポーネントのルールがこのタイプを分類するか、どのフィールドを読むかを記述します。
サポート分類がないタイプでは「なし」と記載します。

## 検証

| 動作 | 実機（デバイス名） | フィクスチャのみ |
| --- | --- | --- |
| <behaviour> | <device> | |
