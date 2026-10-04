# <Type name>

<!-- One paragraph: what this type covers and which devices are in its class. -->

- **類型權杖：** `<type>`
- **提供者：** `<provider name>`
- **重新掃描觸發條件：** `<uevent subsystems>`

## 識別

說明 `id` 如何由哪些穩定屬性組成，以及它如何在重新插拔、重新啟動和重新編號後
保持穩定。範例：`<type>:<stable key>`。

## 詳細資料

| 欄位 | 類型 | 永遠存在 | 含義 | 來源 |
| --- | --- | --- | --- | --- |
| `<field>` | 字串 | 是 | <what it means> | <sysfs file, ioctl, vendor API> |

## 記錄範例

```json
{"id": "<type>:...", "type": "<type>", "provider": "...", "<type>": {}}
```

## 涵蓋的變化

列出此類別裝置的差異，以及如何處理每項差異（數量、格式、範圍、選用欄位、
複合裝置、多個相同裝置、使用中會變更的值）。

## 支援規則

說明 Neat 元件的規則是否分類此類型，以及它們讀取哪些欄位。若此類型沒有
支援分類，請寫「無」。

## 驗證

| 行為 | 真實硬體（哪個裝置） | 僅固定資料 |
| --- | --- | --- |
| <behaviour> | <device> | |
