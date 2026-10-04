# 加入裝置類型

本頁供希望讓 Sentinel 探索新種類裝置的貢獻者使用，涵蓋檢查核心介面、
撰寫提供者、測試，以及記錄新類型，讓應用程式開發者能使用它。

## 1. 檢查核心是否描述裝置

每個提供者都是 Sentinel 內讀取核心介面的 Rust 程式碼：sysfs、`/proc`、
uevent，以及唯讀 ioctl（V4L2、ALSA、IIO、媒體控制器等）。Sentinel 不會
執行其他程式或載入 libcamera、GStreamer 等使用者空間堆疊來探索裝置。
開始前，請確認核心會公開應用程式需要知道的裝置資訊。

相機提供者是可用的範例：USB 相機位於 `src/peripherals/v4l2/`，MIPI 相機
位於 `src/peripherals/mipi/`。

## 2. 每個提供者都須遵守的規則

1. **唯讀。** 查詢裝置；不得設定、串流、取得所有權或變更核心狀態。
   以 `O_RDONLY | O_NONBLOCK` 開啟裝置節點。執行中的應用程式不得察覺探索作業。
2. **穩定識別。** 記錄的 `id` 必須在重新插拔、重新啟動和重新編號後保持相同。
   由穩定屬性（匯流排拓撲、序號、核心實體名稱）組成，絕不能使用
   `/dev/videoN`、卡片編號或列舉順序。以類型為前綴：`camera:...`、`microphone:...`。
3. **涵蓋整個類別，而非手邊樣本。** 桌上的裝置是第一個測試固定資料，不是
   規格。撰寫程式碼前，列出該類別其他成員的差異（數量、格式、範圍與離散值、
   缺少選用欄位、複合裝置、同時存在多個相同裝置、使用中會改變的值）並逐一處理。
4. **降級而非失敗。** 遺漏的選用欄位就省略。無法讀取的選用部分在記錄中說明
   原因。只有在提供者無法產生正確清單時才讓掃描失敗；Sentinel 會保留上次正確
   記錄並顯示錯誤。
5. **只報告事實。** 回報裝置與核心提供的資訊。Neat 元件是否支援裝置由該元件
   的支援規則決定，而非提供者。
6. **工作有界。** 提供者在守護程式內執行且無法終止，因此絕不能阻塞，損壞或
   惡意裝置也不能讓它永久工作。每個列舉迴圈最多 1024 個項目
   (`MAX_ENUMERATION_ENTRIES`)，每個裝置每次掃描最多 4096 次查詢
   (`EnumerationBudget`、`MAX_DEVICE_ENUMERATIONS`，位於
   `src/peripherals/videodev2.rs`)。

## 3. 記錄

提供者會傳回記錄清單：

```json
{"id": "microphone:usb-1-2.3:1.2", "type": "microphone",
 "provider": "daemon.audio.alsa", "details": {"channels": 2, "formats": ["S16_LE"]}}
```

| 欄位 | 規則 |
| --- | --- |
| `id` | 非空白、在所有提供者間唯一且穩定（見上文） |
| `type` | 小寫字母、數字、`_` 或 `-`，以字母開頭；最多 64 個字元；不可為 `id`、`type` 或 `provider` |
| `provider` | 提供者名稱，例如 `daemon.audio.alsa` |
| `details` | JSON 物件。其欄位是此類型的結構描述，記錄於[裝置類型](device-types/README.md) |

在目錄中，`details` 會發布在以類型命名的鍵下：
`{"id", "type", "provider", "microphone": {...}}`。用戶端將未知類型當成
JSON 讀取，所以類型無需其他變更就會出現在 API、CLI、Insight 和 Neat Core
的 `details` 中。

## 4. 撰寫提供者

1. 建立 `src/peripherals/<name>/`（或 `<name>.rs`），並實作
   `src/peripherals/model.rs` 的 `Provider` trait：

   ```rust
   impl Provider for MicrophoneProvider {
       fn name(&self) -> &str { "daemon.audio.alsa" }
       // Kernel uevent subsystems that should trigger a rescan.
       fn subsystems(&self) -> &[String] { &self.subsystems } // ["sound"]
       fn discover(&mut self) -> Result<Vec<Record>, ProviderError> { ... }
   }
   ```

2. 註冊它：在 `src/peripherals/mod.rs` 的 `builtin_providers()` 中加入一行。
3. 讓檔案系統根目錄可注入（例如 `with_roots(sys, dev)`），並將 ioctl 呼叫放在
   小型 trait 後方，讓測試不需硬體即可執行。相機提供者展示了此模式。
4. 將失敗對應至 `ProviderError` 代碼：`io.permission_denied`、`io.open` 或
   `peripherals.discovery_failed`。

## 5. 測試

像 Sentinel 一樣執行一次提供者，查看它實際加入目錄的內容：

```bash
simaai-sentinel peripherals --test-provider daemon.audio.alsa
```

該指令會使用與守護程式相同的逐提供者檢查驗證記錄、套用支援規則、列印結果，
並在失敗時以非零狀態結束。ID 在提供者之間也必須唯一：以類型和提供者專用鍵
作為前綴。

必須有單元測試：

- 規則 3 中列出的每個變化軸各一個測試；
- 若有真實裝置擷取，使用由其轉錄的固定資料，否則使用格式忠實的合成固定資料，
  並標示來源；
- 錯誤路徑：權限遭拒、裝置在掃描途中消失、格式錯誤的回應。

建立提取要求前，請在本機執行儲存庫的 CI 步驟：
`cargo fmt --check`、`cargo check --locked`、`cargo test --locked` 和
`scripts/build_vulcan_package.sh`。

## 6. 記錄類型

從[範本](device-types/TEMPLATE.md)建立
`docs/peripherals/device-types/<type>.md`，並將它列於
[裝置類型索引](device-types/README.md)。應用程式開發者依賴此頁讀取記錄。
請說明哪些行為在真實硬體上驗證，哪些只用固定資料驗證。

## 檢查清單

- [ ] 核心會公開應用程式所需的裝置資訊
- [ ] 唯讀；不設定、不串流、不取得所有權
- [ ] 由穩定屬性產生穩定 `id`，並以類型為前綴
- [ ] 已列出變化軸，且每個軸都有測試
- [ ] 選用資料會降級而非導致失敗
- [ ] 已檢查 `--test-provider` 輸出
- [ ] 已加入裝置類型頁面並列入索引
- [ ] 儲存庫 CI 步驟已在本機通過
