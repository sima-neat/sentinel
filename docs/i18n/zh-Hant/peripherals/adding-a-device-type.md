# 新增裝置類型

Sentinel 的周邊裝置目錄列出硬體事實。
新的裝置種類（例如顯示器或序列轉接器）需要一個具型別的記錄、一個探索它的提供者、
測試，以及此資料夾中的一個頁面。
應用程式是否支援某個裝置由該應用程式決定，而不是由 Sentinel 決定。
唯一的例外是攝影機模式是否可用（請參閱[目錄](../api.md)）。

## 1. 定義記錄

在 `src/peripherals/` 下新增一個模組，為該裝置定義一個 `serde` 結構，
並在 `src/peripherals/mod.rs` 中將其加入為 `Peripheral` 的一個變體。
變體名稱會成為 JSON 中的 `type` 標籤，
因此 `Display(display::Display)` 會序列化為 `{"type": "display", ...}`。
請為新變體擴充 `Peripheral::id`、`kind` 與 `describe`。

- `id` 必須在重新插拔與重新開機後保持穩定：請從匯流排路徑、
  序號或其他在這些情況下仍不變的屬性推導，
  絕不可使用列舉索引，例如音效卡編號或 `/dev/videoN`。
- `id` 也必須在整個目錄中唯一；工作執行緒不會
  進行檢查。請如現有 id 一樣，以類型與提供者作為前綴
  （`camera:v4l2:…`、`microphone:alsa:…`），其餘部分則由兩個相同裝置不可能共用的資訊組成，
  例如它們所插入的連接埠。
  單憑序號並不足夠：廉價裝置經常重複使用同一個序號。
- 選用的事實以 `Option` 欄位表示，並標記
  `#[serde(default, skip_serializing_if = "Option::is_none")]`，
  如同現有記錄的做法，讓未知的事實不出現在 JSON 中。
  若只用單純的 `Option`，則會序列化為 `null`。
- 回報核心或裝置所公開的事實。不要新增用來表達
  策略的欄位，例如某個模式是否受支援。

## 2. 撰寫提供者

實作 `Provider` trait：

```rust
pub trait Provider: Send {
    fn name(&self) -> &'static str;            // e.g. "display.drm"
    fn subsystems(&self) -> &'static [&'static str]; // uevent subsystems, e.g. ["drm"]
    fn discover(&mut self) -> Result<Vec<Peripheral>, ProviderError>;
}
```

`discover` 會在 `peripherals` 執行緒上執行一次完整掃描。
工作執行緒會在啟動時、收到來自 `subsystems` 之一的 uevent 後，以及重新整理時呼叫它。

- **唯讀。** 以唯讀方式開啟裝置節點，且只使用查詢介面。
  絕不佔用、設定裝置，也不從裝置串流。
- **有上限。** 為讀取的每個清單與每個查詢迴圈設定上限，讓故障的
  裝置無法使掃描停滯。
  `videodev2.rs` 中的 V4L2 輔助函式示範了這種模式。
- **錯誤。** 掃描無法完成時，傳回 `ProviderError`，並附上代碼，例如 `io.permission_denied`
  或 `io.open`（見 `sysutil.rs`）。接著目錄會保留此提供者上次正常的裝置，
  並在 `errors` 中列出該錯誤。
  在掃描期間消失的裝置會被略過，不算錯誤；
  其移除所送出的 uevent 會觸發重新掃描。
- **降級處理，不要失敗。** 缺少的選用屬性只會讓其欄位被省略。
  對於存在但無法讀取的屬性，依其用途處理：
  - 掃描需要用來尋找或識別裝置的屬性（其 id、其 USB
    識別資訊）屬於錯誤，因此會保留該提供者上次正常的裝置；
  - 僅用來補充某項事實的屬性，仍會發布該裝置，只是不含該事實，
    並在記錄中附上原因
    （麥克風記錄的 `issues` 對無法讀取的擷取中繼資料就是這樣處理）。

在 `src/peripherals/mod.rs` 的 `builtin_providers()` 中註冊此提供者。

## 3. 測試

測試讀取的是測試夾具樹，而非即時系統：
將 sysfs、procfs 與 `/dev` 的根目錄作為參數傳給提供者，並模擬 ioctl 層，
就像 `alsa/tests.rs` 與 `v4l2/tests.rs` 的做法。
請涵蓋此類裝置之間的各種差異，而不只是手邊的裝置：
數量、格式、範圍與離散值、缺少的選用欄位、同時存在多個相同裝置，
以及掃描途中消失的裝置。請斷言兩個相同裝置會得到不同的 id。
從真實硬體複製而來的測試夾具，
請標示為真實擷取資料。

`cargo test --locked` 必須通過，且 `cargo fmt --check` 與 `cargo clippy
--locked --all-targets` 不得回報任何新問題。

## 4. 撰寫文件

在此資料夾中新增一個頁面，列出提供者名稱、
uevent 子系統、`id` 的推導方式、每個欄位及其出現時機、
錯誤代碼，以及一個 JSON 範例。從 [API 參考文件](../api.md)
與 [文件索引](../README.md) 連結到此頁面，然後重新產生翻譯
（見 [在地化](../i18n/README.md)）。
