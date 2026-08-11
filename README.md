# generate_voice_from_voicepeak

VOICEPEAK の VPP プロジェクトから、音声・VPP由来ラベル・SBV2互換情報・MFA用カタカナ転写を生成する Rust CLI です。

主な処理は次の通りです。

```text
VPP
 ↓
sentence-list の token.syl[].s から確定読みを復元
 ↓
句読点・記号を保持した連結カタカナ
 ↓
1 WAV = 1 LAB（人工的な空白なし）
 ↓
MFA Japanese tokenizer
 ↓
japanese_katakana_mfa
 ↓
japanese_mfa acoustic model
 ↓
phone timing
```

SBV2 の phone、tone、word2ph、VPP 音素長、アクセント、イントネーション、VOICEPEAK の runtime 応答も `labels.jsonl` に保存します。

## 前提条件

- Windows
- Rust toolchain（`cargo`）
- VOICEPEAK 本体
- 解析対象の `.vpp` ファイル
- 音声生成時は VOICEPEAK の named pipe に接続できること
- MFA alignment を行う場合は MFA 3.x と日本語モデル

このリポジトリの既定 VPP パスは次です。

```text
voicepeak.vpp
```

別の VPP を使う場合は、コマンドの第 1 引数で指定してください。

## ビルド

```powershell
cargo build --release
```

実行ファイルは次に生成されます。

```text
target\release\generate_voice_from_voicepeak.exe
```

開発時は `cargo run --release -- ...` でも実行できます。

## 基本実行

VPP の全 block を処理する例です。

```powershell
cargo run --release -- `
  "voicepeak.vpp" `
  dataset `
  --variants 15
```

実行ファイルを直接使う場合:

```powershell
.\target\release\generate_voice_from_voicepeak.exe `
  "voicepeak.vpp" `
  dataset `
  --variants 15
```

`--variants 15` は、1 block あたり 15 variant を生成します。速度グループは 5 種類なので、各速度に 3 variant ずつ割り当てられます。

既定の速度グループ:

```text
0.750, 0.875, 1.000, 1.125, 1.250
```

### 生成計画だけ確認

VOICEPEAK を起動・接続せず、VPP と生成数だけ確認します。

```powershell
cargo run --release -- `
  "voicepeak.vpp" `
  dataset `
  --variants 15 `
  --dry-run
```

### 先頭 block だけ処理

```powershell
cargo run --release -- `
  "voicepeak.vpp" `
  dataset-smoke `
  --max-blocks 5 `
  --variants 5
```

`--variants` は速度数 5 の倍数でなければなりません。

### block を選択して検証

長時間の全量生成を避け、任意の 0-based block だけを処理できます。

```powershell
cargo run --release -- `
  "voicepeak.vpp" `
  dataset-cherry-smoke `
  --blocks 0,14,79,99 `
  --variants 5
```

`--blocks` の値は重複不可です。`--blocks` と `--max-blocks` は同時に指定できません。

## CLI オプション

```text
Usage: generate_voice_from_voicepeak [VPP_PATH] [OUTPUT_DIR] [OPTIONS]

--variants N       1 block あたりの総 variant 数。5 の倍数。既定値: 15
--max-blocks N     先頭から N block だけ処理
--blocks LIST      指定した 0-based block だけ処理。例: 0,14,79,99
--strict           最初の音声合成・edit response エラーで停止
--dry-run          VOICEPEAK を起動せず生成計画を表示
-h, --help         ヘルプ表示
```

位置引数を省略した場合:

```text
VPP_PATH    voicepeak.vpp
OUTPUT_DIR  dataset
```

## 出力構造

```text
dataset/
├─ manifest.json
├─ metadata.json
├─ mfa/
│  ├─ custom_words.txt
│  └─ custom.dict                 # MFA CLI が利用可能な場合
├─ speed_0.750/
│  ├─ labels.jsonl
│  ├─ metadata.sbv2.jsonl
│  ├─ requests.jsonl
│  ├─ rejects.jsonl
│  └─ wav/
│     ├─ b000_v000.wav
│     └─ b000_v000.lab
├─ speed_0.875/
├─ speed_1.000/
├─ speed_1.125/
└─ speed_1.250/
```

### `.lab`

各 WAV に対応する `.lab` を `speed_*/wav/` に出力します。

- 1 WAV = 1 LAB
- 1 行の連結カタカナ文字列
- token 境界用の空白は追加しない
- VPP の sentence 順序を維持
- 句読点・記号を保持
- VPP 上 pause 扱いでも、`ッ` などのカタカナ表記は保持
- ひらがなの空読み表記はカタカナへ正規化（例: `ぇ` → `ェ`）

例:

```text
ドオスンノ、コノオミセ。カンッゼンニカンコドリガナイチャッテルジャナイ。
```

読みの復元元は VPP の `sentence-list[].tokens[].syl[].s` です。`jp_g2p` で読みを再推定する処理は行いません。

### `labels.jsonl`

1 行 1 utterance です。主なフィールド:

- `source`: VPP パス、block、variant、速度
- `audio`: WAV のサンプルレート、チャンネル、長さ
- `synthesis`: text、narrator、params、emotions、変化量、再生用 VPP tokens
- `sbv2`: normalized text、phones、tones、word2ph
- `mfa`: `.lab` の相対パス、連結カタカナ、pause 数、警告
- `phone_labels`: VPP 音素・runtime duration・accent・intonation の対応
- `runtime`: VOICEPEAK edit response の取得状態

### `metadata.json`

MFA 用の全 utterance 情報をまとめたファイルです。

- VPP の SHA-256 とバージョン
- `.lab` 生成ポリシー
- utterance ごとの `katakana`
- token ごとの surface、reading、pause、warning
- `custom_words.txt` の語彙数
- MFA 辞書生成の状態

### `requests.jsonl`

各サンプルに実際に渡した VOICEPEAK playback payload を保存します。再現性確認やデバッグに使用できます。

### `rejects.jsonl`

`--strict` なしで処理を継続した場合の synthesis / edit response エラーを保存します。

## MFA 辞書と alignment

MFA の公式モデルをインストールします。

```powershell
mfa model download acoustic japanese_mfa
mfa model download dictionary japanese_mfa
mfa model download g2p japanese_katakana_mfa
```

データ生成時に `mfa` CLI が PATH に存在すると、次を自動実行します。

```text
mfa g2p <OUTPUT_DIR>/mfa/custom_words.txt \
  japanese_katakana_mfa \
  <OUTPUT_DIR>/mfa/custom.dict \
  --sorted
```

生成状態は `manifest.json` の `mfa.dictionary.status` で確認できます。

not_available  MFA CLI が PATH にない
failed         MFA CLI はあるが G2P 実行に失敗
generated      custom.dict を生成済み

`custom_words.txt` は VPP 由来の辞書入力語彙です。`.lab` に空白を挿入するためのものではありません。.lab の tokenization は MFA Japanese tokenizer に委譲します。

MFA alignment の基本形:

```powershell
mfa align `
  dataset\speed_0.750 `
  japanese_mfa `
  japanese_mfa `
  aligned\speed_0.750 `
  --g2p_model_path japanese_katakana_mfa
```

実際の corpus 配置や MFA のバージョンに応じて、MFA の tokenizer / dictionary 設定を確認してください。まず `manifest.json` と `metadata.json` で `mfa_warnings` が 0 であることを確認してから alignment を実行してください。

## 検証

Rust の検査:

```powershell
cargo test
cargo clippy -- -D warnings
cargo build --release
```

block を絞った音声生成の検証例:

```powershell
cargo run --release -- `
  "voicepeak.vpp" `
  dataset-cherry-smoke `
  --blocks 0,14,79,99 `
  --variants 5
```

生成後に確認する項目:

1. `manifest.json` の `counts.failed` が 0
2. `manifest.json` の `counts.mfa_warnings` が 0
3. `.wav` と `.lab` の ID が一致
4. `.lab` に人工的な空白がない
5. `labels.jsonl` の `mfa.katakana` と `.lab` の本文が一致
6. `mfa/custom_words.txt` が重複・空行なし
7. `manifest.json` の `mfa.dictionary.status` が想定どおり

## 注意事項

- 全量生成は block 数 × variant 数 × 速度別 synthesis のため時間がかかります。最初は `--blocks` で検証してください。
- `--strict` は synthesis / edit response のエラーに対する設定です。読み警告は `metadata.json` と `manifest.json` で確認してください。
- `custom.dict` は MFA CLI と `japanese_katakana_mfa` が利用可能な場合だけ生成されます。
- 生成物は `.gitignore` の `dataset*` パターンで Git 管理対象外です。
- このツールは学習素材を生成します。MFA alignment、音響モデル学習、SBV2 モデル学習は別工程です。

## 参考資料

- [Japanese MFA dictionary](https://mfa-models.readthedocs.io/en/latest/dictionary/Japanese/Japanese%20MFA%20dictionary%20v2_0_0.html)
- [Japanese Katakana MFA G2P model](https://mfa-models.readthedocs.io/en/latest/g2p/Japanese/Japanese%20%28Katakana%29%20MFA%20G2P%20model%20v3_0_0.html)
- [MFA pronunciation dictionary generation](https://montreal-forced-aligner.readthedocs.io/en/latest/user_guide/workflows/dictionary_generating.html)
