# generate_voice_from_voicepeak

VOICEPEAK の VPP プロジェクトから、音声・VPP由来ラベル・SBV2互換情報・MFA用カタカナ転写を生成する Rust CLI です。

主な MFA 処理は次の通りです。

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

Julius は同じ VPP 確定読みを使い、公式 segmentation-kit の `yomi2voca()` に phone 変換を委譲します。

```text
VPP 確定カタカナ
 ↓
ひらがな化 + 内部句読点を sp 化
 ↓
segment_julius.pl / yomi2voca()
 ↓
Julius monophone acoustic model
 ↓
phone timing
```

SBV2 の phone、tone、word2ph、VPP 音素長、アクセント、イントネーション、VOICEPEAK の runtime 応答も `labels.jsonl` に保存します。

## 前提条件

- Windows
- Conda
- Rust toolchain（`cargo`）
- VOICEPEAK 本体
- 解析対象の `.vpp` ファイル
- MFA alignment を行う場合は MFA 3.4.1、PostgreSQL、日本語モデル
- Julius forced alignment を行う場合は ffmpeg と Perl（どちらも PATH に必要）

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

## VPP を指定するだけの一括実行

リリース ZIP に同梱される `generate_from_vpp.bat` は、VPP のパスだけを指定して次を順番に実行します。

1. VOICEPEAK で音声を合成
2. Julius 用 16 kHz WAV と VPP 確定カタカナ転写を生成
3. 転写をひらがな + `sp` 形式へ変換
4. 公式 Julius segmentation-kit の `yomi2voca()` で phone 化
5. Julius で強制音素アライメントを実行
6. `dataset\julius\speed_*\wav\*.lab` を時間付きラベルへ置換

```bat
generate_from_vpp.bat "C:\path\to\voicepeak.vpp"
```

出力先は既定で次になります。

```text
C:\path\to\voicepeak_dataset\
```

出力先を明示する場合だけ第 2 引数を追加できます。

```bat
generate_from_vpp.bat "C:\path\to\voicepeak.vpp" "D:\datasets\voicepeak"
```

BAT は `generate_voice_from_voicepeak.exe`、`julius\bin\julius.exe`、日本語モノフォン音響モデル、同梱した `third_party\segmentation-kit\segment_julius.pl` を BAT 自身のディレクトリから解決します。`ffmpeg.exe` と `perl.exe` は PATH に必要です。VOICEPEAK は通常のインストール場所から VPSDK が検出します。

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
├─ speed_1.250/
└─ julius/
   └─ speed_*/
      ├─ wav/
      │  ├─ b000_v003.wav
      │  ├─ b000_v003.lab          # alignment 後は時間付き phone label
      │  ├─ b000_v003.txt          # segmentation-kit に渡したひらがな + sp
      │  └─ b000_v003.julius.log
      └─ phones/
         └─ b000_v003.txt          # 互換・診断用の直接 phone 列
```

### `.lab`

通常の生成では各 WAV に対応する `.lab` を `speed_*/wav/` に出力します。

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

### Julius 用 WAV と強制アライメント

`ffmpeg` が PATH に存在する場合、各 VOICEPEAK WAV から Julius 用の派生 WAV を自動生成します。元の WAV は変更しません。

```text
dataset/julius/speed_0.750/wav/b000_v003.wav
dataset/julius/speed_0.750/wav/b000_v003.lab
```

派生 WAV の形式は Julius の標準日本語 GMM-HMM に合わせて固定します。

```text
sample rate: 16,000 Hz
channels:    mono
sample format: signed 16-bit PCM (pcm_s16le)
```

内部では次と同等の変換を行います。

```text
ffmpeg -hide_banner -loglevel error -y -i input.wav -vn -ar 16000 -ac 1 -c:a pcm_s16le -f wav output.wav
```

通常の CLI 実行直後は `dataset/julius/speed_*/wav/*.lab` に VPP 由来の連結カタカナ転写が入っています。`generate_from_vpp.bat` は `align_julius.ps1` を呼び出し、次の変換を行います。

```text
ドオスンノ、コノオミセ。カンッゼンニカンコドリガナイチャッテルジャナイ。
 ↓
どおすんの sp このおみせ sp かんっぜんにかんこどりがないちゃってるじゃない
 ↓
segment_julius.pl の yomi2voca()
 ↓
Julius phone sequence
 ↓
forced alignment
```

`ッ` は句読点ではないため `sp` へ変換されません。したがって `カンッゼン` は `かんっぜん` のまま segmentation-kit に渡され、`yomi2voca()` が `っ -> q` を処理します。文末の `。` は trailing `sp` にせず、segmentation-kit が自動挿入する `silE` に任せます。

alignment 前に作ったひらがな転写は WAV と同じ場所へ保存されます。

```text
dataset/julius/speed_0.750/wav/b000_v003.txt
```

alignment 後の `.lab` は WaveSurfer Label 形式です。

```text
0.0000000 0.1325000 silB
0.1425000 0.1725000 e
...
```

各 WAV の隣に `*.julius.log` も保存されます。時間変換は公式 segmentation-kit の 10 ms frame + 12.5 ms offset の処理をそのまま使用します。

公式 `segment_julius.pl` は古い shell command を含むため、`align_julius.ps1` は各 speed group の WAV/TXT を一時 staging directory にコピーして実行します。これによりユーザーの dataset path に空白が含まれていても、そのパスを Julius の legacy command line に直接渡しません。

`ffmpeg` が利用できない場合も通常の WAV、LAB、SBV2、MFA 出力は継続します。この状態は `manifest.json` の `julius.status` と各 `labels.jsonl` の `julius.status` に `not_available` として記録します。一括 BAT は `ffmpeg` または Perl がない場合に開始前エラーとします。

Julius 用派生 WAV の生成状態は `manifest.json` で確認できます。

- `generated`: 全サンプルの派生 WAV を生成済み
- `failed`: `ffmpeg` は利用できたが、一部の変換に失敗
- `not_available`: `ffmpeg` が PATH に存在しない

読みの復元元は VPP の `sentence-list[].tokens[].syl[].s` です。Julius alignment 用の読みを `jp_g2p` で再推定しません。

### Julius phone labels

`julius/speed_*/phones/*.txt` には、既存互換・診断用途として SBV2 phone 列から変換した Julius phone 列も保存します。

```text
dataset/julius/speed_0.750/phones/b000_v003.txt
silB e q sp s o sp silE
```

この phone 列は `labels.jsonl` の `julius.phone_status`、`phones_path`、`phones`、`phone_line`、`lexical_phone_line` にも保存されます。ただし、`generate_from_vpp.bat` の正式な forced alignment 入力はこのファイルではありません。正式経路は VPP 確定カタカナ → `wav/*.txt` → segmentation-kit `yomi2voca()` です。

### `labels.jsonl`

1 行 1 utterance です。主なフィールド:

- `source`: VPP パス、block、variant、速度
- `audio`: WAV のサンプルレート、チャンネル、長さ
- `synthesis`: text、narrator、params、emotions、変化量、再生用 VPP tokens
- `sbv2`: normalized text、phones、tones、word2ph
- `mfa`: `.lab` の相対パス、連結カタカナ、pause 数、警告
- `phone_labels`: VPP 音素・runtime duration・accent・intonation の対応
- `julius`: 派生 WAV/LAB の状態と診断用 phone sequence
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

## GitHub Actionsリリース

`tag` は `Cargo.toml` の version と一致している必要があります。現在の version では `v0.1.0` または `0.1.0` を使用します。`-rc.1` のような suffix は prerelease として扱われます。
`v*` または数字で始まる tag を push すると、`.github/workflows/release.yml` が Windows x64 向けリリースを作成します。

```powershell
git tag v0.1.0
git push origin v0.1.0
```

CI は Julius transcript conversion の回帰テストを実行してからリリース ZIP を生成し、GitHub Release に添付します。

Julius は MinGW ではなく MSVC ベースでビルドし、vcpkg の zlib は静的リンクします。Julius と grammar-kit のコミットは workflow に固定し、生成した zip の `julius/BUILD-INFO.txt` に記録します。公式 segmentation-kit の `segment_julius.pl` と MIT License も `third_party/segmentation-kit/` に同梱します。

```text
generate_voice_from_voicepeak/
├─ generate_voice_from_voicepeak.exe
├─ generate_from_vpp.bat
├─ align_julius.ps1
├─ README.md
├─ scripts/
│  └─ julius-transcript.ps1
├─ third_party/
│  └─ segmentation-kit/
│     ├─ segment_julius.pl
│     └─ License.md
└─ julius/
   ├─ BUILD-INFO.txt
   ├─ JULIUS-LICENSE
   ├─ bin/
   │  ├─ julius.exe
   │  ├─ julius-simple.exe
   │  ├─ mkbingram.exe
   │  └─ *.exe / *.dll
   └─ grammar-kit/
      ├─ model/phone_m/
      │  ├─ hmmdefs_monof_mix16_gid.binhmm
      │  ├─ hmmdefs_ptm_gid.binhmm
      │  └─ logicalTri
      └─ SampleGrammars/
```

同梱した Julius を直接実行する場合は、grammar-kit を current directory にして設定ファイルの相対パスを保ちます。

```powershell
Push-Location .\julius\grammar-kit
..\bin\julius.exe -C .\hmm_ptm.jconf -input rawfile -filelist .\files.txt
Pop-Location
```

このアプリのデータ生成では Julius 用 WAV 変換に `ffmpeg`、公式 segmentation-kit 実行に Perl を使用します。どちらもリリース ZIP には含めず PATH から検出します。通常 CLI は ffmpeg がなくても通常の WAV、LAB、SBV2、MFA、診断用 phone 列の生成を継続します。`generate_from_vpp.bat` は ffmpeg または Perl がない場合に開始前エラーとし、アライメント未実行のまま終了します。

## MFA 3.4.1 + PostgreSQL

今回の固定構成は **Windows + Conda + MFA 3.4.1 + MFA 管理 PostgreSQL + 日本語モデル** です。MFA のデータベースをバージョン間で混在させないため、専用の root directory を使います。

### 環境構築

```powershell
conda create -n aligner -c conda-forge `
  montreal-forced-aligner=3.4.1 `
  postgresql `
  spacy `
  sudachipy `
  sudachidict-core `
  -y

conda activate aligner
mfa version                         # 3.4.1
postgres --version
```

MFA の root directory を設定します。PowerShell を開き直した後も使う場合は User 環境変数として登録してください。

```powershell
$env:MFA_ROOT_DIR="$HOME\Documents\MFA_341_PG"

[Environment]::SetEnvironmentVariable(
  "MFA_ROOT_DIR",
  "$HOME\Documents\MFA_341_PG",
  "User"
)
```

PostgreSQL backend と自動 server 起動を MFA の既定値にします。

```powershell
mfa configure --enable_use_postgres --enable_auto_server
mfa server init --use_postgres --verbose
```

MFA が管理する PostgreSQL はローカル用途の一時的な backend です。通常は `auto_server` により各コマンドの開始・終了時に自動管理されます。DB を作り直す場合だけ次を実行します。

```powershell
mfa server delete --use_postgres
mfa server init --use_postgres --verbose
```

MFA 3.4.1 の SQLite backend では `word_interval_temp` が存在しないという既知 issue が open になっているため、このプロジェクトでは alignment に PostgreSQL を使用します（[issue #965](https://github.com/MontrealCorpusTools/Montreal-Forced-Aligner/issues/965)）。

### 日本語モデル

```powershell
mfa model download acoustic japanese_mfa
mfa model download dictionary japanese_mfa
mfa model download g2p japanese_katakana_mfa
```

役割は次の通りです。

```text
japanese_mfa dictionary
  └─ OOV → japanese_katakana_mfa G2P
japanese_mfa acoustic model
  └─ forced alignment
```

このプロジェクトは VPP から確定カタカナ読みを復元するため、標準読みの再推定ではなく `japanese_katakana_mfa` を OOV fallback として使います。

### `.lab` の規則

`.lab` は WAV と同じ basename の UTF-8 テキストで、1 行の連結カタカナにします。

```text
b000_v003.wav
b000_v003.lab
```

```text
ドオスンノ、コノオミセ。カンッゼンニカンコドリガナイチャッテルジャナイ。
```

- VPP の `tokens[].syl[].s` を発話順に連結する
- token 境界の人工的な空白を入れない
- `、。？！…・` などの句読点・記号は保持する
- VPP に保存された実発音の `ッ`、長音などを標準読みへ戻さない
- `.lab` に `pau` / `sil` を書かない。無音区間は MFA の alignment で扱う
- 読みが空の非句読点 token は黙って削除せず、`metadata.json` の warning として確認する

### `custom.dict`

データ生成時に `mfa` CLI が PATH に存在すると、VPP 由来語彙から次を自動生成します。

```text
mfa g2p <OUTPUT_DIR>/mfa/custom_words.txt \
  japanese_katakana_mfa \
  <OUTPUT_DIR>/mfa/custom.dict \
  --sorted
```

`custom_words.txt` は辞書入力語彙であり、`.lab` に空白を挿入するためのものではありません。alignment では標準の `japanese_mfa` dictionary と `--g2p_model_path japanese_katakana_mfa` を使用します。

生成状態は `manifest.json` の `mfa.dictionary.status` で確認できます。

```text
not_available  MFA CLI が PATH にない
failed         MFA CLI はあるが G2P 実行に失敗
generated      custom.dict を生成済み
```

### alignment

まず `--blocks` または `--max-blocks` で少量の WAV/LAB を生成し、alignment を確認します。

```powershell
cargo run --release -- `
  "voicepeak.vpp" `
  dataset-smoke `
  --max-blocks 1 `
  --variants 5
```

MFA 3.4.1 の基本 alignment:

```powershell
$CORPUS="$PWD\dataset-smoke\speed_1.125\wav"
$OUTPUT="$PWD\aligned-smoke"

mfa align `
  $CORPUS `
  japanese_mfa `
  japanese_mfa `
  $OUTPUT `
  --g2p_model_path japanese_katakana_mfa `
  --single_speaker `
  --use_postgres `
  --clean `
  --overwrite `
  --output_format long_textgrid `
  --verbose
```

`mfa align` の引数順は `CORPUS_DIRECTORY DICTIONARY_PATH ACOUSTIC_MODEL_PATH OUTPUT_DIRECTORY` です。プログラムから読む場合は `--output_format json` を使えます。

`mfa validate` は WAV/LAB 対応、OOV、入力形式の確認用です。G2P fallback は `mfa align --g2p_model_path ...` で適用されるため、最終評価は `align` の出力で行います。

smoke test の acceptance criteria:

1. すべての WAV に同名 `.lab` がある
2. alignment 結果の `phones` tier が生成される
3. 通常発音部分に `spn` がない
4. `ッ`、`ン`、長音、句読点前後の境界を数件目視確認する

出力形式や backend の詳細は [MFA alignment](https://montreal-forced-aligner.readthedocs.io/en/v3.4.1/user_guide/workflows/alignment.html)、[MFA configuration](https://montreal-forced-aligner.readthedocs.io/en/v3.4.1/user_guide/configuration/index.html)、[MFA servers](https://montreal-forced-aligner.readthedocs.io/en/v3.4.1/user_guide/server/index.html) を参照してください。

## 検証

Rust の検査:

```powershell
cargo test
cargo clippy -- -D warnings
cargo build --release
```

Julius 転写変換の回帰テスト:

```powershell
.\scripts\test-julius-transcript.ps1
```

このテストには、次のケースを含めています。

- `カンッゼンニ。` → `かんっぜんに`（`ッ` を `sp` にしない）
- 内部句読点 → 1 個の `sp`
- 文末句読点 → trailing `sp` なし
- 長音 `ー` を保持して `yomi2voca()` に渡す
- `ヴ` 系を upstream が期待する `う゛` 表記へ変換

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
8. Julius 実行後の `julius/speed_*/wav/*.txt` が VPP 読みに対応する
9. Julius 実行後の `.lab` に `silB` / phone / `silE` の時間付き行がある

## 注意事項

- 全量生成は block 数 × variant 数 ×速度別 synthesis のため時間がかかります。最初は `--blocks` で検証してください。
- `--strict` は synthesis / edit response のエラーに対する設定です。読み警告は `metadata.json` と `manifest.json` で確認してください。
- `custom.dict` は MFA CLI と `japanese_katakana_mfa` が利用可能な場合だけ生成されます。
- Julius の正式 alignment 経路は公式 segmentation-kit の `yomi2voca()` を使用するため Perl が必要です。
- 生成物は `.gitignore` の `dataset*` パターンで Git 管理対象外です。
- このツールは学習素材を生成します。MFA alignment、音響モデル学習、SBV2 モデル学習は別工程です。

## 参考資料

- [Julius segmentation-kit](https://github.com/julius-speech/segmentation-kit)
- [Japanese MFA dictionary](https://mfa-models.readthedocs.io/en/latest/dictionary/Japanese/Japanese%20MFA%20dictionary%20v2_0_0.html)
- [Japanese Katakana MFA G2P model](https://mfa-models.readthedocs.io/en/latest/g2p/Japanese/Japanese%20%28Katakana%29%20MFA%20G2P%20model%20v3_0_0.html)
- [MFA pronunciation dictionary generation](https://montreal-forced-aligner.readthedocs.io/en/latest/user_guide/workflows/dictionary_generating.html)
