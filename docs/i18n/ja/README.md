# Sentinelのドキュメント

Sentinelは、バックグラウンドデーモンでModalix DevKitのテレメトリを収集し、
アトミックなJSONキャッシュを書き込み、そのキャッシュをターミナルレポートで表示します。
これらのページでは、各パネルが何を報告するか、各値がどこから取得されるか、
どのように計算されるか、およびその制限事項について説明します。

## 運用ビューのパネル

| パネル | ドキュメント | 目的 |
| --- | --- | --- |
| 概要 | [概要パネル](panels/overview.md) | 熱、電力、CPU、メモリ、MLAメモリ、ストレージ、およびネットワークの主要な状態。 |
| サーマル | [サーマルパネル](panels/thermal.md) | ダイ上のRTSN/PVTサイトと、ボードレベルのハードウェアモニターの測定値。 |
| 電源 | [電源パネル](panels/power.md) | PMBusのレール電力、合計値、セッションの平均値/ピーク値、およびコレクターの状態。 |
| システム | [システムパネル](panels/system.md) | 全体/コアごとのCPU、ロードアベレージ、Linuxメモリ、MLA割り当て、EV74 CMAメモリ、およびプロセス。 |
| ストレージ/ネットワーク | [ストレージとネットワークパネル](panels/storage-network.md) | eMMC/NVMeの容量とI/O、およびネットワーク全体のトラフィック。 |

## レポートと収集動作

- [測定モデル](measurement-model.md)では、コレクターの実行間隔、キャッシュ履歴、
  チャートのセマンティクス、閾値、古いデータ、利用できない値、
  リセット動作について説明します。
- [レポートとJSONエクスポート](reports.md)では、キャッシュスキーマと、
  `table`、`export`、`sensors`、`status`の各コマンドについて説明します。
- [ローカルエージェント API](api.md)では、ライブ読み取りと、
  デーモンのUnixソケットを介したトレース制御について説明します。
- [実行結果の記録と比較](run-comparison.md)では、永続的な
  チェックポイント、Compare Runsタブ、保持期間、およびCSV/JSONエクスポートについて説明します。

## 周辺機器

Sentinelは、ボードに接続されているカメラとマイクも一覧表示します。
[ローカルエージェント API](api.md#peripherals)でカタログとその更新方法を説明しています。
デバイスタイプごとに専用のページがあります：

- [USBカメラ](peripherals/camera.md)（V4L2）
- [MIPI CSI-2カメラ](peripherals/camera-mipi.md)
- [マイク](peripherals/microphone.md)（ALSA）
- [ボードのカメラ構成](peripherals/board.md)：モデル、カメラ
  オーバーレイ、および構成済みのセンサーとサポート対象のセンサー
- コントリビューター向けの[デバイスタイプの追加](peripherals/adding-a-device-type.md)
