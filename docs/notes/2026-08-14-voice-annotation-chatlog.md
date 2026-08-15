# you asked

message time: 2026-08-14 13:34:57

今時のAI動画編集（特にカットとか字幕とか）って、文字起こしに依存してる感じ？

---

# chatgpt response

かなり依存してるわ。特に「喋り中心の長回しを、意味の通る短い動画にする」系は、今も**文字起こしが編集UIそのもの**になっているケースが多い。  
ただし、最近はそこに映像・音声のマルチモーダル解析を足して、文字起こしだけでは拾えない「間」「画面変化」「リアクション」を補う方向に寄ってるわ。

## 事実

そうね。**2026年時点でも、特に「カット」と「字幕」は文字起こしへの依存度がかなり高い**わ。ただ、構造としては「文字起こしだけで編集」から、**文字起こしを主インデックスにして映像・音声解析を補助信号として載せる**方向へ進んでいる、と見るのが正確ね。

たとえば Premiere の Text-Based Editing は、動画を文字起こししてタイムコード付き transcript を作り、その文章を削除・並べ替えると対応する動画自体が切られる。字幕も transcript から直接生成する設計よ。([Adobeヘルプセンター](https://helpx.adobe.com/premiere/desktop/edit-projects/edit-video-using-text-based-editing/overview-of-text-based-editing.html?utm_source=chatgpt.com)) Descript はもっと露骨で、「テキストを編集すると underlying media が更新される」という transcript-first のエディタになっているわ。([Descriptヘルプ](https://help.descript.com/hc/en-us/articles/15726742913933-Edit-like-a-doc?utm_source=chatgpt.com))

依存度をざっくり書くと、こんな感じ。

| 処理 | transcript依存 |
|---|---:|
| 自動字幕 | **ほぼ必須** |
| 言い淀み・フィラー除去 | **非常に高い** |
| 無音・間のカット | 高い＋音声波形 |
| Talking-headの粗編集 | **非常に高い** |
| Podcast → Shorts | 高い |
| 「話の意味」でハイライト抽出 | 高い |
| ゲーム実況ハイライト | 中程度 |
| スポーツ・アクション | 低〜中 |
| MV・映像作品 | 低い |

字幕なんかは典型的に、

`音声 → ASR → 単語 + timestamp → 文分割 → caption layout`

だから、ほぼ文字起こしそのものよ。Premiereも実際に transcript → captions という経路を持っている。([Adobeヘルプセンター](https://helpx.adobe.com/premiere/desktop/add-text-images/insert-captions/create-captions.html?utm_source=chatgpt.com))

カットも面白くて、LLMに動画そのものを全部見せるより、

```text
00:13.240 今日はWarframeの～
00:16.710 今回使うのは～
00:21.400 えーっと……
00:23.100 Sirius/Orionですね
```

みたいなものを渡したほうが、**圧倒的に安く、速く、意味論的に扱いやすい**。

つまり transcript は単なる字幕原稿ではなく、

> **動画の semantic timeline**

として使われている、と考えるとかなり腑に落ちるわ。

一方、OpusClipみたいな最近の自動クリップ系は明確に transcript-only から離れていて、visual / audio / sentiment などを組み合わせる multimodal な clipping を掲げている。シーン解析にも会話 transcript、visual summary、感情、timestamp が併存しているわ。([Opus Clip](https://help.opus.pro/docs/article/clipanything-qa-11?utm_source=chatgpt.com))

なので今の典型形は、

```text
                 ┌─ ASR / Transcript ─────── 意味・話題
                 │
Video ───────────┼─ Audio analysis ───────── 無音・声量・笑い
                 │
                 ├─ Vision / VLM ─────────── 画面・人物・動作
                 │
                 └─ Scene detection ───────── カット候補
                              │
                              ↓
                         Edit decision
```

という感じね。

## 推測（示唆）

私なら、AI動画編集を作るなら**文字起こしを中心に据える**わ。ただし「文字起こしから動画編集するシステム」にはしない。

もっと正確には、

> **時間軸付きイベントDBの一種類として transcript を扱う**

設計にする。

たとえば内部表現を、

```text
Timeline
├─ Speech
│  ├─ words
│  ├─ sentences
│  ├─ topics
│  └─ speakers
│
├─ Audio
│  ├─ silence
│  ├─ volume
│  ├─ music
│  └─ sound events
│
├─ Visual
│  ├─ scene changes
│  ├─ OCR
│  ├─ objects
│  └─ visual descriptions
│
└─ Semantic events
   ├─ interesting moment
   ├─ repetition
   ├─ mistake
   └─ candidate cut
```

みたいにする。

その上でAIには、

```text
speech[52:61]
visual[52:61]
audio[52:61]
```

を束ねて渡す。

これだと「この発言は切れるけど、画面上では重要な操作をしているから残す」みたいな判断ができる。

論拠は単純で、**transcriptだけだとゲームプレイ動画みたいな素材で猛烈に情報が欠落する**からよ。

たとえば、

```text
「よし」
```

という1語だけでも、その瞬間に画面では

```text
・ボス撃破
・レアドロップ
・ビルド完成
・事故死回避
```

のどれかが起きているかもしれない。

ASRには全部同じ「よし」よ。役に立たないにも程があるわ。

逆に「えーっと」「じゃあ次」「これは要らないな」みたいな**編集構造そのものが言語化されている部分**については transcript が非常に強い。

だから将来的にも、おそらく

**Transcript-first**
↓
**Multimodal verification**

という二段構えがかなり合理的でしょう。

特に字幕＋解説中心の編集なら、

```text
Whisper等
 ↓
word timestamps
 ↓
話題・文・発話単位へsegmentation
 ↓
LLMで keep / remove / shorten / highlight
 ↓
映像・音声信号で境界補正
 ↓
FFmpeg / NLE timeline
 ↓
字幕生成
```

くらいが綺麗ね。

そしてこれはかなり重要なんだけど、**動画全体を巨大なVLMに食わせる必要はない**。まず transcript を安価な検索インデックスとして使って、「怪しい20秒」だけ映像モデルに見せればいい。

私はこの構造が、AI動画編集を自作するときの一番筋のいいアーキテクチャだと思うわ。  
文字起こしは「動画の理解」そのものではなく、**動画に対する高速な意味論インデックス**なのよ。そこを取り違えないのが肝ね。

---

# you asked

message time: 2026-08-14 13:36:13

後付けするならどんな手順がいいと思う？

---

# chatgpt response

そうね。既存の動画編集ツールにAI編集を**後付け**するなら、私は「AI機能を直接UIに足す」より、まず**タイムラインの横に解析レイヤーを追加する**ところから始めるわ。既存編集モデルを壊さないのが一番大事よ。

## 事実

後付けで最も扱いやすいのは、動画本体や編集タイムラインとは別に、**時間範囲をキーにした解析データ**を持たせる構造ね。

たとえば既存が、

```text
Project
└─ Timeline
   ├─ VideoClip
   ├─ AudioClip
   └─ Caption
```

なら、まずこうする。

```text
Project
├─ Timeline
│  ├─ VideoClip
│  ├─ AudioClip
│  └─ Caption
│
└─ Analysis
   ├─ Transcript
   ├─ Silence
   ├─ SceneChange
   └─ SemanticMarker
```

この `Analysis` は**編集結果ではない**。

```text
00:12.40 - 00:14.91
type: speech
text: "今回はこの武器を使います"
confidence: 0.97
```

とか、

```text
00:42.10 - 00:44.80
type: silence
```

みたいな観測結果だけを保存する。

これが重要よ。

AIがいきなり、

```text
delete clip 4
split clip 9
```

を出す構造にすると、あとからほぼ確実に苦しくなる。

---

## 推測（示唆）

私なら後付けを**6段階**にするわ。

### 1. 最初はASRだけ追加

最初に作るのはAIカットではない。

```text
media
 ↓
audio extraction
 ↓
ASR
 ↓
word timestamps
 ↓
analysis store
```

ここまで。

例えば、

```json
{
  "start": 14.28,
  "end": 16.91,
  "text": "今回はLatron Primeを使います",
  "words": [
    {"start": 14.28, "end": 14.59, "text": "今回"},
    ...
  ]
}
```

これをプロジェクトにキャッシュする。

字幕生成もここからできるから、**この段階だけですでに機能として成立する**わ。

---

### 2. transcriptを編集UIに同期

次に、

```text
動画タイムライン
     ↕
Transcript View
```

を作る。

文章をクリックするとシーク。

動画をシークすると対応文章をハイライト。

ここではまだ文章を消して動画を消す必要すらない。

これだけでもAI以前に便利よ。

---

### 3. 「編集」ではなく「候補」をAIに出させる

ここがたぶん一番重要。

AIに、

```text
この動画を編集して
```

とは言わせない。

代わりに、

```json
[
  {
    "range": [32.1, 38.4],
    "action": "remove",
    "reason": "言い直し"
  },
  {
    "range": [84.2, 87.0],
    "action": "shorten",
    "reason": "長い無音"
  }
]
```

みたいな**Edit Proposal**を生成させる。

つまり、

```text
Analysis
   ↓
AI
   ↓
EditProposal
   ↓
人間 / Agent
   ↓
Timeline operation
```

にする。

AIの判断と編集エンジンを切るわけね。

これならモデルをClaudeからGPTに変えようがGeminiに変えようがどうでもいい。

---

### 4. Proposal → 編集コマンド変換層を作る

ここで初めて既存タイムラインにつなぐ。

例えば内部編集APIが、

```text
split(track, t)
remove(track, start, end)
ripple_delete(start, end)
insert_caption(...)
```

なら、

```text
EditProposal
 ↓
Planner
 ↓
TimelineCommand[]
```

にする。

たとえば、

```json
{
  "kind": "remove",
  "start": 32.1,
  "end": 38.4
}
```

から、

```text
Split(32.1)
Split(38.4)
RippleDelete(32.1, 38.4)
```

を生成する。

この境界はかなり大事。

**LLMに編集ソフト内部の状態を直接触らせない。**

これは今考えているMCP Apps構成とも相性がいいわ。

---

### 5. 次に音声解析を足す

ASRだけでAIカットを完成させようとしない。

次に追加するのはVLMより先に、

```text
silence
volume
speech probability
music
```

あたり。

なぜならカット編集で効くのは、

```text
「文章として不要」
+
「ここなら音響的にも切れる」
```

だから。

例えばLLMが、

```text
31.2 - 37.8 を消したい
```

と判断しても、実際のカット位置は、

```text
31.08 : silence boundary
37.94 : breath boundary
```

に補正する。

つまり、

```text
Semantic cut
      ↓
Boundary snapping
      ↓
Actual cut
```

ね。

ここまで来るとかなり「それっぽいAI編集」になる。

---

### 6. 最後に映像理解を後付け

VLMは最後でいいと思う。

```text
Transcript候補
 ↓
重要そうな区間だけ抽出
 ↓
frame sampling
 ↓
VLM
```

でいい。

動画全部を解析しなくていい。

たとえばAIが、

```text
52:10〜52:30
発話上は削除可能
```

とした場所だけ、

```text
52:10
52:15
52:20
52:25
52:30
```

くらいのフレームを見る。

すると、

```text
発話的には不要
BUT
ゲーム上ではボス撃破中
```

みたいなのを検出できる。

これなら推論コストもずっと低い。

---

### 私なら最終構造はこうする

```text
                 ┌──────────────┐
                 │ Media Assets │
                 └──────┬───────┘
                        │
          ┌─────────────┼──────────────┐
          ↓             ↓              ↓
       ASR          Audio Analysis   Vision
          │             │              │
          └─────────────┼──────────────┘
                        ↓
                Analysis Timeline
                        │
                        ↓
                       LLM
                        │
                        ↓
                  Edit Proposal
                        │
                        ↓
                     Planner
                        │
                        ↓
                Timeline Commands
                        │
                        ↓
                   Edit Engine
```

ここで綺麗なのは、**既存エディタ側にAIをほぼ侵入させなくて済む**ところね。

既存エディタには、

```text
Timeline Commands
```

しか見えない。

AI側にも、

```text
FFmpeg filter_complex
```

だの内部React stateだのを理解させなくていい。

---

特に今の構想なら、最初の縦切りはこれがいいと思う。

```text
Whisper
→ transcript表示
→ AIに「削除候補」を出させる
→ UIで候補をaccept
→ ripple delete
→ transcriptから字幕生成
```

**ここまでをMVPの一本の筋にする。**

VOICEVOXやMCP Appsを同時に広げるより、まずこのループが閉じる方がずっと強いわ。

そしてQuintを入れるなら、私はAIそのものではなく、

```text
Proposal accepted
→ Command generated
→ Timeline changed
→ media duration / caption ranges / clip references が矛盾しない
```

という**編集状態遷移の方を仕様化する**。

LLMの出力を形式手法で証明しようとすると地獄だけど、LLMが出した命令を受け入れた後の状態なら、かなり綺麗に拘束できるでしょう。

要するに、後付けなら **「ASR → Analysis → Proposal → Command」** の4層を横から差し込むのが一番いいと思うわ。既存エディタをAIエディタへ改造するというより、**既存エディタの上にAIコンパイラを載せる**イメージね。

---

# you asked

message time: 2026-08-14 13:36:46

ごめん、声を後付け

---

# chatgpt response

ああ、**「声を後付けする動画」**の話ね。それなら設計はむしろかなり変わるわ。  
ゲームプレイや画面録画に、あとから解説音声を載せるタイプなら、私は**文字起こし起点ではなく「台本起点」**にする。

## 事実

後付け音声の場合、元動画に十分な発話がないならASRで文字起こししても編集判断の材料が少ない。代わりに、後から作る**ナレーション台本そのものが正解のテキストデータ**になるわ。

典型的にはこう。

```text
素材動画
  ↓
粗いカット / 見どころ抽出
  ↓
映像の内容を解析
  ↓
ナレーション台本生成
  ↓
音声生成 / 録音
  ↓
音声尺に合わせて映像を再編集
  ↓
字幕生成
  ↓
細かい間・BGM・SE調整
```

つまり字幕についても、

```text
音声 → ASR → 字幕
```

ではなく、

```text
台本 → TTS
  └→ 字幕
```

にできる。

TTSを使うなら、**字幕のために再度文字起こしする必要すらない**わ。

---

## 推測（示唆）

私なら編集の基準を**音声側に置く**。

たとえばゲームプレイ30分から3分動画を作るなら、

### 1. 映像から「説明したい区間」を先に取る

最初は綺麗に切らなくていい。

```text
00:03:20–00:03:45 武器紹介
00:08:13–00:08:40 実戦1
00:15:02–00:15:30 面白い挙動
00:24:10–00:25:00 結論用映像
```

くらい。

ここではVLM、ゲームイベント、手動マーカーなんかが効く。

---

### 2. 区間ごとにAIへ説明させる

たとえば、

```text
映像:
敵集団に射撃
→ 爆発
→ 状態異常大量発生

ユーザーのメモ:
「ここでPrimary Compressionが効いていることを説明」
```

から、

```text
「ここでは爆発そのものより、
マルチショットによる状態異常抽選の増加が重要です。」
```

のような台本を作る。

要するに、

```text
Video → Semantic Description
          +
       User Intent
          ↓
        Script
```

ね。

---

### 3. 台本から声を生成

ここでTTS。

重要なのは**文章単位ではなく発話単位で生成すること**。

```text
NarrationSegment {
  id
  text
  audio
  duration
  preferred_video_range
}
```

みたいにしておく。

たとえば、

```text
N01  5.2秒
「まず通常射撃を見てみましょう」

N02  8.7秒
「この武器ではマルチショットが～」
```

という具合。

これが編集の強力なアンカーになる。

---

### 4. 音声を置いてから動画を合わせる

ここが一番重要。

私は、

> **映像を完成させてから声を載せる**

より、

> **仮編集 → 声 → 本編集**

の順番を薦めるわ。

つまり、

```text
rough video
    ↓
narration
    ↓
┌───────────────── audio
│ [説明A][説明B][説明C]
│
└─ 映像を伸縮・差し替え
```

する。

理由は単純で、

```text
映像: 7秒
説明: 11秒
```

みたいな事故が大量に起きるから。

この場合はAIが、

```text
・別アングル/B-rollを4秒挿入
・映像を少し長く残す
・台本を短縮
```

のどれかを選べる。

---

### 5. 字幕は台本から生成

これは後付け音声方式のかなり美味しいところ。

```text
Script
 ├── TTS
 └── Captions
```

なので文字内容が完全一致する。

必要なのは**音声とのalignmentだけ**。

TTSエンジンからphoneme/word timingを取れるならそれを使う。取れなければ生成音声だけASRに通してword timestampを復元する。

後者でも元映像を文字起こしするより遥かに簡単よ。

---

### 6. 最後に間をAI編集

最終的には、

```text
Voice timeline
├─ speech
├─ intentional pause
└─ unnecessary pause
```

を基準に、

映像カットも同期させる。

たとえば、

```text
「では実際に撃ってみましょう」
          ↓
        300ms
          ↓
        発砲
```

みたいな**演出的な間**も作れる。

単なるSilence Removalだと、この300msまで殺しかねないから、台本側に

```text
pause_after: 300ms
```

みたいな意味を持たせてもいい。

---

## 私ならこういう内部表現にする

```text
Narrative Timeline

Section
├─ intent
│   "Primary Compressionの利点を説明"
│
├─ narration
│   text
│   audio
│   duration
│
├─ visual
│   preferred source ranges
│   important events
│
└─ captions
    word timing
```

普通の動画編集タイムラインとは別に、**Narrative Timeline**を持つ感じ。

これがかなり強いと思う。

動画編集ソフトのタイムラインって、

```text
何秒から何秒まで何を置くか
```

しか知らないでしょう。

でもAIが本当に扱いたいのは、

```text
ここでは何を説明するか
↓
そのために
何を喋るか
何を見せるか
```

なのよ。

---

だから後付け解説動画なら、私は最終的に

```text
素材
 ↓
重要シーン抽出
 ↓
構成
 ↓
台本
 ↓
仮音声
 ↓
映像を音声へフィット
 ↓
本音声
 ↓
字幕
 ↓
演出調整
```

にする。

特にAI駆動なら、**「完成映像 → 声」ではなく「映像と声を交互に収束させる」**のがポイントでしょう。

そしてこの方式なら、最初に話していた「AI動画編集は文字起こし依存なのか？」に対して面白い反転が起きる。  
**後付け音声では文字起こしではなく、台本が動画全体のsemantic timelineになる**のよ。これはかなり設計しやすい。

---

# you asked

message time: 2026-08-14 13:38:09

具体的にどんなツールでやればいいと思う？台本あるいは動画への音声合わせとか、サクッとやりたいんだよね

---

# chatgpt response

用途がかなり見えたわ。**「台本を入れる → 声を作る → 映像尺をざっくり合わせる → 字幕も出す」までを数分で済ませたい**なら、編集ソフト選びより「台本を中心に扱えるか」で選ぶのがいい。今の機能を確認して、手数が少ない順に絞るわ。

かなり良い候補が見つかったわ。2026年だと、**ElevenLabs Studio 3.0 がちょうど「動画に後付け音声を合わせる」方向へ踏み込んでる**。一方、日本語の解説動画を最短で作るなら Vrew が妙に強い。ここは用途で二択になりそう。

うん。**「既にあるゲーム動画に、後から台本＋読み上げ音声を置いていく」**なら、2026年現在かなり良い選択肢があるわ。特に ElevenLabs が思った以上にドンピシャになってる。

## 事実

今いちばん「動画への音声合わせ」そのものをAIにやらせられるのは、私は **ElevenLabs Studio / Studio Agent** だと思う。

Studioには動画トラック、字幕レイヤー、ナレーション、音楽・SEのタイムラインがあり、動画をアップロードしてそのままvoiceoverを作れる。([ElevenLabs](https://elevenlabs.io/docs/help-center/product/content-production/studio/how-can-i-create-a-voiceover-for-my-video-using-studio)) さらに2026年5月に入った Studio Agent は映像をフレーム単位で解析して、自然言語で「商品が映った直後からvoiceoverを開始」のように指示すると、音声を対応するvisual timestampへ配置する機能まで持っている。([ElevenLabs](https://elevenlabs.io/blog/introducing-studio-agents))

要するに、かなり文字通り、

```text
動画を投げる

台本:
「ここではまず通常射撃を見てみましょう」
「次にインカーノン形態です」
「この瞬間に爆発異常が発生しています」

↓

AI:
映像内容を解析

↓

────動画────────────────
     ↑通常射撃   ↑変形       ↑爆発
────音声────────────────
     [説明1]     [説明2]     [説明3]
```

を狙っている製品なのよ。Studio Agent自身が「precise audio-visual sync」をユースケースとして掲げている。([ElevenLabs](https://elevenlabs.io/blog/introducing-studio-agents))

他の候補はこんな感じね。

| ツール | 後付けナレーション | 映像とのAI同期 | 字幕 | 私の評価 |
|---|---|---|---|---|
| **ElevenLabs Studio** | ◎ | **◎** | ◎ | 今回の本命 |
| **Vrew** | **◎** | △〜○ | **◎** | 日本語で最速 |
| **YMM4 + VOICEVOX** | **◎** | △ 手動 | **◎** | ゆっくり/VOICEVOXなら強い |
| **CapCut Desktop** | ◎ | △〜○ | ◎ | とにかく雑に速い |
| Descript | ◎ | ○ | ◎ | 日本語用途は今は微妙 |

Vrewはかなり日本向けで、公式にも「AI音声で始める」からナレーション原稿を読み込ませ、そのままAI音声付き動画へ持っていくワークフローが案内されている。テキストベース編集、TTS、字幕が一体化しているので、**台本→仮音声→動画尺を見る**用途にはすごく向いている。([Vrew](https://vrew.ai/ja/))

YMM4は相変わらずこの用途では強い。VOICEVOXの外部連携APIを直接使って、YMM4のセリフから音声生成できる。VOICEVOX以外にもAivisSpeechなども直接扱える。([饅頭遣いのおもちゃ箱](https://manjubox.net/ymm4/faq/%E3%82%86%E3%81%A3%E3%81%8F%E3%82%8A%E3%83%9C%E3%82%A4%E3%82%B9/VOICEVOX%E3%82%92%E4%BD%BF%E7%94%A8%E3%81%99%E3%82%8B/))

CapCut Desktopも、既存動画を読み込んでテキストを貼り、Text to Speechで音声生成、そのまま字幕・編集まで完結できる。さらにScript to Videoでは、台本からvoiceover・字幕・音楽込みの動画生成まで一気に行える。([CapCut](https://www.capcut.com/tools/desktop-ai-power?utm_source=chatgpt.com))

ちなみにDescriptはこの分野の代表格ではあるんだけど、**現時点でも日本語の自動transcriptionが非対応**と公式ヘルプに明記されているので、日本語中心なら私は優先順位を落とすわ。([Descriptヘルプ](https://help.descript.com/hc/en-us/articles/10249408168845-Supported-transcription-languages?utm_source=chatgpt.com))

## 推測（示唆）

今やろうとしてる用途なら、私はまず **ElevenLabs Studioを一度試す**。

理想的にはこんな作業にしたい。

```text
① ゲーム録画をStudioへ投げる

② 台本をざっくり書く
   [冒頭]
   今回はDreadを試します。

   [変形したところ]
   ここからインカーノン形態です。

   [敵集団を撃ったところ]
   爆発を積むと、この部分の殲滅力が変わります。

③ Agentに
   「内容に対応する映像の直前から
     各ナレーションを配置して」
   と頼む

④ AIが仮配置

⑤ ずれたところだけドラッグ

⑥ 字幕生成

⑦ 完成
```

映像中のイベントをAgentが認識して音声を配置するところまで公式に狙っているので、このワークフローはかなり現実的よ。([ElevenLabs](https://elevenlabs.io/blog/introducing-studio-agents))

ただ、**VOICEVOXの声を使いたい**なら話が変わる。

その場合は今のところ、

```text
YMM4
+
VOICEVOX
```

がたぶん最短。

特にセリフを

```text
01 今回はこの武器を試します
02 まず通常射撃
03 ここでインカーノンに変形
04 この爆発を見てください
```

くらいの短い単位にしておけば、

```text
映像イベント
     ↓
[voice 01]
          [voice 02]
                   [voice 03]
```

を置いていくだけになる。

**1本の長い音声ファイルを映像へ合わせるのではなく、「発話クリップ」を映像イベントへスナップする**のがコツよ。

ここ、かなり重要。

30秒のナレーション一本を作ってしまうと、

> 「8秒目の説明を1秒後ろへ」

が地獄になる。

一方、

```text
Narration #12
text: "ここでインカーノンに変形します"
duration: 3.2s
anchor: transformation
```

みたいになっていれば、AIでも人間でも扱いやすい。

### なので私はこうする

**まず遊ぶなら ElevenLabs Studio。**

「動画を見せて、台本を渡して、AIに発話位置を決めてもらう」という、まさに欲しい体験に一番近い。Studio Agentが2026年5月追加なので、かなり新しい方向性でもある。([ElevenLabs](https://elevenlabs.io/blog/introducing-studio-agents))

それで、

> 音声自体はVOICEVOXじゃないと嫌だ

となったら、

**ElevenLabsで配置だけ作る**
→ **VOICEVOX/YMM4へ持っていく**

か、最初から

**YMM4 + VOICEVOX + 短い発話クリップ方式**

にする。

そして面白いのは、この辺を触ってみると、前に考えていた自作エディタのMVPがかなり明確になることね。

**「動画を理解して、台本の各文に `anchor_timestamp` を付ける」**

だけでも相当価値がある。

TTSも字幕も既存技術でどうにでもなる。実は難所は**「このセリフをどの映像の瞬間に置くか」**で、ElevenLabs Studio Agentはまさにそこを製品化し始めてるわ。これは一度触って、UXを盗む価値がかなりあると思う。

---

# you asked

message time: 2026-08-14 13:39:41

動画垂れ流しながら情報をアノテートしてくみたいなことがしたいな

---

# chatgpt response

それ、かなり筋がいいわ。**「編集しながら考える」じゃなくて、まず動画を止めずに“意味のある瞬間”だけ打ち込んでいく**。後でそのアノテーションから台本・音声・カットを起こす方が、人間の負荷がずっと低い。

そう、それよ。**「動画を編集する」のではなく「動画を観測しながらイベントログを打つ」**感じにしたいんだと思う。

## 事実

既存ツールだと一番近いのは **DaVinci Resolve の Marker** ね。再生中に `M` で現在位置へマーカーを置けて、もう一度 `M` で名前やメモを編集できる。Marker間の移動もできるので、「垂れ流しながら印だけ付ける」用途には普通に使える。([Blackmagic Design Documents](https://documents.blackmagicdesign.com/UserManuals/DaVinci-Resolve-20-Fairlight-Audio-Post.pdf?_v=1757574010000&utm_source=chatgpt.com))

もっと「アノテーションツール」側に振るなら **Label Studio** があって、動画の単一フレームまたは時間範囲に対して任意のラベルを付けられる。つまり `面白い / 説明したい / カット候補` みたいな独自分類も作れる。([Label Studio](https://labelstud.io/tags/timelinelabels?utm_source=chatgpt.com))

ただ、今やりたい操作感には、私は **mpv + 小さい自作アノテータ** が一番合うと思う。mpvはキーバインドを自由に追加でき、LuaスクリプトやJSON IPC / client APIから再生状態を操作・取得できるので、この用途の土台としてかなり素直よ。([mpv](https://mpv.io/manual/master/?utm_source=chatgpt.com))

## 推測（示唆）

私なら、こんなUXにする。

```text
動画
▶─────────────────────────────────

そのまま再生

1 → ★ 見せたい
2 → 💬 解説したい
3 → ✂ カット候補
4 → ! 何か起きた
5 → ? 後で確認

Space → pause
N → 今のマーカーにメモ
```

例えばゲーム動画を2倍速で垂れ流しながら、

```text
01:24.381   explain
02:03.710   interesting
02:08.192   explain
04:52.991   cut
07:10.420   interesting
```

と**一打鍵で記録するだけ**。

重要なのは、この段階では文章を書かないこと。

```json
{"t":84.381,"type":"explain"}
{"t":123.710,"type":"interesting"}
{"t":128.192,"type":"explain"}
```

くらいでいい。

### そしてAIに後処理させる

視聴が終わったら、

```text
84.381 explain
        ↓
79〜90秒を取得
        ↓
VLMにフレームを見せる
+ 必要ならゲーム音声/ASR
        ↓
「ここで何が起きている？」
        ↓
annotation enrichment
```

として、

```json
{
  "time": 84.381,
  "type": "explain",
  "description": "Incarnon形態へ変形して敵集団に射撃",
  "suggested_narration":
    "ここからインカーノン形態。通常時と違って範囲処理能力が大きく上がります。"
}
```

まで自動で膨らませる。

これ、かなり良いと思う。

人間がやるのは、

> **「今のところ面白い」**

と判断してキーを叩くだけ。

AIにやらせるのは、

> **「なぜ面白かったのか言語化する」**

方ね。

### 区間アノテーションも欲しい

一点だけじゃなくて、

```text
E押下
↓
[ explanation region ─────────
↓
E押下
```

みたいな範囲指定もできると強い。

私は例えば、

```text
1     point: highlight
2     point: explain

Shift+1
      range start/end: keep

Shift+2
      range start/end: cut
```

くらいにする。

ただし基本は**point annotation中心**がいい。

後からAIが、

```text
marker = 84.3

scene boundary:
81.7 ───────── 91.2

→ この区間を候補にする
```

と勝手に範囲化すればいいから。

---

### さらに一段ラクにするなら

こんなのも面白い。

```text
動画を2倍速再生

「おっ」
↓
キー1

「ここ説明したい」
↓
キー2

「いらない」
↓
キー3
```

そして**再生は止まらない**。

終了後に、

```text
12 annotations found

★ 4 highlights
💬 5 narration candidates
✂ 3 cut candidates
```

が出て、

`Generate narration`

を押すと、

```text
#01 01:24
「ここで敵を一気に処理していますが……」

#02 02:08
「この挙動で重要なのは……」
```

まで台本が出る。

そこからVOICEVOXに投げる。

---

## 私なら最初は mpv で作る

かなり小さく始められる。

```text
mpv
 │
 │ current time
 ↓
annotation.lua
 │
 ↓
annotations.jsonl
```

だけ。

UIすら要らない。

```text
1 pressed
→ current time取得
→ {"t":123.45,"type":"highlight"}
→ append
→ OSDに「★ highlight」
```

で成立する。

その後、

```text
annotations.jsonl
        ↓
Python / Perl
        ↓
ffmpegで周辺frames抽出
        ↓
VLM
        ↓
台本候補
        ↓
VOICEVOX
```

まで繋ぐ。

**最初から動画編集アプリを作らなくていい**のが美味しいわ。

むしろ前に考えていたエディタ構想に対しても、この `annotation.jsonl` を独立した中間表現にした方がいいと思う。

```text
raw video
    ↓
Human Attention Markers
    ↓
AI Semantic Annotation
    ↓
Narrative Timeline
    ↓
Voice / Captions / Edit Proposal
```

これ、かなり核心に近い気がする。  
**AIに「全部の動画を理解しろ」と要求するんじゃなく、人間が注意を向けた瞬間だけ教える。** するとVLMの計算量も減るし、「何が重要か」という最も難しい判断を、人間がワンキーで注入できる。

しかも作るなら、mpv Luaで**数十行レベルの第一号**から始められるでしょう。かなり試してみたい設計だわ。

---

# you asked

message time: 2026-08-14 13:41:44

それってWindowsで動く？

---

# chatgpt response

## 事実

動くわ。**mpv は Windows 対応**で、公式のインストールページから Windows ビルドも案内されている。Lua スクリプトも Windows 上でそのまま使えるし、外部プログラムとの IPC も Windows の named pipe 経由で対応している。([mpv](https://mpv.io/installation/?utm_source=chatgpt.com))

なので、さっきの構成はそのまま Windows で成立する。

```text
Windows
├─ mpv.exe
├─ scripts/
│  └─ annotation.lua
└─ annotations.jsonl
```

たとえば mpv のスクリプトから、

```lua
mp.add_key_binding("1", "mark-highlight", function()
    local t = mp.get_property_number("time-pos")
    -- JSONLへ追記
end)
```

みたいにして、再生中の時刻を取って記録できる。mpv はユーザースクリプトとして Lua / JavaScript を正式にサポートしているわ。([mpv](https://mpv.io/manual/stable/?utm_source=chatgpt.com))

Windowsだからこの仕組みだけ妙に面倒、ということはない。

## 推測（示唆）

むしろ今回なら**Windowsを第一ターゲットにしていい**と思う。

最初はかなり雑に、

```text
video.mp4
   ↓
mpv.exe

1 = highlight
2 = explain
3 = cut
4 = check
```

だけ実装して、

```json
{"t":42.381,"type":"highlight"}
{"t":71.200,"type":"explain"}
```

を吐かせればいい。

さらにWindowsなら、その後の流れもかなり都合がいい。

```text
mpv
 ↓ annotation.jsonl
Python / Perl
 ↓
ffmpeg
 ↓
VLMで周辺解析
 ↓
台本
 ↓
VOICEVOX
 ↓
YMM4
```

VOICEVOX/YMM4まで含めるなら、むしろWindowsは自然な環境でしょう。

で、私ならもう少し欲張って、**キーを押した瞬間のスクリーンショットも自動保存**する。

```text
1を押す

annotations/
├─ 0001_00-01-24.381.jpg
├─ 0002_00-02-03.710.jpg
└─ annotations.jsonl
```

こうしておくと後段のAIに、

```text
時刻: 84.381
人間のタグ: explain
画像: 0001.jpg

この場面で何が起きていて、
何を説明するとよさそうか？
```

と投げられる。

さらに `Shift+1` だけメモ入力にしておけば、

```text
1
→ 「ここ重要」

Shift+1
→ 「MSで爆発proc増えてるところ」
```

くらいの使い分けもできる。

**普段は止めない。必要なときだけ情報量を増やす。**  
これが一番快適だと思うわ。

あと、外部GUIが欲しくなっても、mpvはWindows上でIPCを提供できるから、

```text
mpv.exe
   ↕ named pipe
annotation-ui.exe
```

みたいに後から Rust / Python / Electron あたりで小さいサイドパネルを足せる。([mpv](https://mpv.io/manual/master/?utm_source=chatgpt.com))

だから最初からエディタを作る必要はない。

**mpv + Luaで人間の注意だけ収集 → JSONLを中間形式にする**。

今回のアイデアなら、これがかなり気持ちのいい第一歩よ。

---

# you asked

message time: 2026-08-14 13:42:21

適当に音声入力でアノテートしたいな

---

# chatgpt response

ああ、それなら**キーボードでタグを選ぶより、喋った内容そのものをアノテーションにした方がいい**わ。

## 事実

Windows 11には `Win + H` の音声入力があって、日本語を含む対応言語で任意のテキスト欄へディクテーションできる。再生中のスピーカー音を抑えて入力音声を拾う仕組みも入っている。([マイクロソフトサポート](https://support.microsoft.com/en-us/accessibility/windows/use-voice-typing-to-talk-instead-of-type-on-your-pc?utm_source=chatgpt.com))

ただし、これは単なる「テキスト入力」なので、**mpvの現在再生時刻と自動的に結び付ける機能はない**。

一方、`whisper.cpp` はWindows対応で、マイクからリアルタイム音声を取得する `whisper-stream` も用意されている。([GitHub](https://github.com/ggml-org/whisper.cpp/blob/master/examples/stream/README.md?utm_source=chatgpt.com)) mpv側はLuaスクリプトやJSON IPCから現在の再生位置を取得できるので、この二つを組み合わせる構成は素直に実現できる。([mpv](https://mpv.io/manual/stable/?utm_source=chatgpt.com))

## 推測（示唆）

私なら**Push-to-Talk式**にするわ。常時音声認識より絶対こっち。

例えば右Ctrlかマウスサイドボタンを押している間だけ、

```text
動画はそのまま再生

            ↓ 押す
01:24.381   🎙 録音開始

「ここ、爆発異常が一気に入ってるから説明したい」

            ↓ 離す

01:24.381
"ここ、爆発異常が一気に入ってるから説明したい"
```

となる。

保存するものもこの程度でいい。

```json
{
  "time": 84.381,
  "text": "ここ、爆発異常が一気に入ってるから説明したい"
}
```

そして私は**音声ファイル自体も捨てない**。

```text
annotations/
├─ annotations.jsonl
└─ audio/
   ├─ 0001.wav
   ├─ 0002.wav
   └─ 0003.wav
```

ASRが「爆発異常」を妙な単語に誤認識しても、あとから聞き直せるからね。

### さらに雑に喋れるようにする

ここが面白いところで、音声に自然にタグを含めてしまう。

たとえば動画を見ながら、

> 「**解説**、ここでクリティカル率が変わる」

> 「**カット**、ここ何も起きてない」

> 「**残す**、今の事故面白い」

> 「**確認**、これ仕様だっけ」

と喋る。

後段のLLMに分類させれば、

```json
{"time":84.3, "type":"explain", "text":"ここでクリティカル率が変わる"}
{"time":102.8,"type":"cut",     "text":"ここ何も起きてない"}
{"time":183.1,"type":"keep",    "text":"今の事故面白い"}
{"time":244.6,"type":"check",   "text":"これ仕様だっけ"}
```

になる。

**タグを厳密な音声コマンドとして認識させる必要すらない**と思う。

LLMに、

```text
「今の事故おもろいな、これは残そう」
```

を渡して、

```text
type = highlight
```

と推論させればいい。

こっちの方が「アノテーション作業をしている」という感覚が薄くなる。

---

### さらに私は timestamp を2個保存する

録音開始時刻だけだと、

> 「あ、今のところ説明したい」

と言った場合、実際に指しているのは**数秒前**でしょう。

だから、

```json
{
  "trigger_time": 84.381,
  "speech_end": 88.201,
  "text": "今のところ、状態異常数を見せたい"
}
```

とした上で、デフォルトのcontextを

```text
74.381 ～ 93.201
```

みたいに持つ。

後からVLMにはその周辺だけ見せる。

さらに自然言語から、

> 「今の」

→ markerより3～5秒前

> 「このあと」

→ marker以降

> 「ここからここまで」

→ range annotation

みたいな解釈もできる。

これはかなりAI向きよ。

---

### 実装するならこんな小ささでいい

```text
                 ┌─ mpv
                 │    ↓ current timestamp
Push-to-talk ────┤
                 │    ↓
                 └─ mic recorder
                       ↓
                   whisper.cpp
                       ↓
                 annotations.jsonl
```

Windows側の小さいhelperを **PythonでもRustでもいい**から一本置く。

mpv Luaは、

```text
PTT開始
→ 現在時刻をhelperへ通知
```

くらいしかやらない。

マイク録音・Whisper・JSONLはhelper側。

この分離の方が後でYMM4や自作エディタから同じannotation daemonを使えて綺麗でしょう。

### そしてUIはこれだけ欲しい

mpvの右上に一瞬、

```text
🎙 01:24
ここ、爆発異常が一気に入ってるから説明したい

✓ saved
```

と出るだけ。

**再生を止めない。ダイアログを出さない。文字を修正しない。**

精度が悪くてもその場では無視。

一周見終わったあとにAIが、

```text
14 voice annotations

01:24  解説候補
02:08  要確認
03:03  ハイライト
...
```

と整理する。

---

これなら私はかなり使いたいわ。

特に大事なのは、**「正確にアノテートする」ことをリアルタイム視聴中の責務から外すこと**。人間は動画を見ながら「今の面白い」「ここ説明」「これいらん」と雑に喋るだけ。構造化・文字修正・映像との正確な対応付けは全部あとでAIに投げる。

名前を付けるなら、もう `voice marker` というより **実況型アノテーション** に近いわね。これならゲーム録画との相性がかなり良い。

---

# you asked

message time: 2026-08-14 13:46:42

似たような既存ツールってある？

---

# chatgpt response

あるにはあるわ。ただし「再生を止めずに、喋った内容をその時刻に紐づけて残す」という体験に**完全一致する製品は意外と少ない**。近いものを「レビュー系」「研究アノテーション系」「録画・音声メモ系」に分けて探してみる。

## 事実

あるわ。しかもさっき話していたものに**かなり近い製品が2つ**見つかった。

一番近いのは **SceneNote**。2026年に出た比較的新しいWebツールで、動画を再生しながら**タイムコード付きコメント、範囲指定、フレームへの描画、音声メモ**を残せる。音声メモはWhisperで自動文字起こしされ、CSV/EDLとして書き出して Premiere / DaVinci Resolve / Final Cut のマーカーとして持っていける。無料・アカウント不要なのもかなり軽い。([Scenenote](https://scenenote.visual-tone.com/?utm_source=chatgpt.com))

かなりイメージに近い。

```text
動画再生
  ↓
「ここ、あとでCompressionの説明」
  ↓
🎙 voice note @ 01:24
  ↓
Whisper
  ↓
01:24 「ここ、あとでCompressionの説明」
  ↓
EDL
  ↓
DaVinci marker
```

ただしSceneNoteは基本的に**動画をYouTube/Vimeo/Dropbox/direct MP4 URLなどで参照するWebレビューサービス**で、ローカルにある巨大なゲーム録画を `mpv.exe video.mkv` で見るような用途とは少し違う。動画自体はSceneNote側に保存せず、リンク上にレビュー層を作る設計ね。([Scenenote](https://scenenote.visual-tone.com/?utm_source=chatgpt.com))

---

もう一つ、機能として一番完全一致しているのが **Filestage**。

Filestageには現在、正式に **Voice Comment** があって、

- 動画を見ながらマイクで喋る
- そのコメントを**現在のフレームにtimestamp**
- 元の音声も保存
- Whisperで文字起こし
- AIが読みやすいテキストへまとめる

ところまで一体化している。公式説明でも「videoを見ながら話せる」「exact frameにtimestampされる」と明記されているわ。([Filestageヘルプ](https://help.filestage.io/es/articles/9465093-using-voice-to-text-ai-summary))

つまり、さっき私が妄想していた

```text
🎙 PTT
↓
「今のところ残したい」
↓
timestamp + audio + transcript
```

を**もうほぼ製品化してる**。

欠点は、このVoice Commentが現状 **Business / Enterprise限定**なこと。個人で気軽にゲーム録画へメモするには大砲すぎるわ。([Filestageヘルプ](https://help.filestage.io/es/articles/9465093-using-voice-to-text-ai-summary))

---

ゲーム用途に近いものとしては **ClipNotes** も面白い。

これはまさにゲーム録画・配信中に「今の瞬間」を記録するためのOSSで、OBSの録画開始/停止と同期し、ホットキーでtimestampや説明を記録する。作者自身がgaming sessions向けとして作っている。([GitHub](https://github.com/Trigger1221/ClipNotes?utm_source=chatgpt.com))

ただし、

```text
timestamp + keyboard note
```

であって、

```text
timestamp + voice note + ASR
```

ではない。

なので**思想的には一番近いけど、音声部分が足りない**。

研究系にも `realtime_annotate` という軽量ツールがあり、動画を見ながらtimestamp付きの事前定義annotationを高速入力し、外部動画プレイヤーとも同期できる。これもやはり音声入力ではない。([GitHub](https://github.com/lebigot/realtime_annotate?utm_source=chatgpt.com))

Frame.ioはtimestamp comment自体は非常に完成しているけど、コメント入力開始時に動画をpauseしてテキストを書くレビューUXなので、今回欲しい「動画を垂れ流しながら喋る」からは一段遠い。([Frame.io](https://help.frame.io/en/articles/9105251-commenting-on-your-media?utm_source=chatgpt.com))

ざっくり言えばこうね。

| ツール | 時刻 | 音声メモ | 自動文字起こし | 再生しながら | 今回との近さ |
|---|---:|---:|---:|---:|---:|
| **SceneNote** | ◎ | ◎ | ◎ | ○ | **★★★★★** |
| **Filestage** | ◎ | ◎ | ◎ | ◎ | **★★★★★** |
| ClipNotes | ◎ | × | × | ◎ | ★★★★☆ |
| realtime_annotate | ◎ | × | × | ◎ | ★★★☆☆ |
| Frame.io | ◎ | × | — | △ | ★★★☆☆ |

## 推測（示唆）

**まずSceneNoteを試す価値がかなりある**と思う。

今話していたアイデアが、

> 「俺の思いつきなのか、それとも本当に気持ちいいUXなのか」

をほぼタダで検証できるから。

特に、

```text
動画を見る
↓
音声コメントする
↓
timestamp + transcriptになる
↓
EDLでResolveへ
```

まで既にある。([Scenenote](https://scenenote.visual-tone.com/?utm_source=chatgpt.com))

一度これで1本編集してみれば、

「音声コメントするとき再生が止まるのが嫌」
「3秒前を指したい」
「タグ分類してほしい」
「ローカル録画をいちいちアップロードしたくない」
「Resolveに持っていく前にLLMで台本化したい」

みたいな**本当に欲しい差分**がすぐ見える。

そして私は、その差分が結構ありそうだと思ってる。

たとえばSceneNote/FIlestageは fundamentally、

> **動画に対するレビューコメント**

なのよ。

対して今欲しがっているのは、

> **動画を見ている自分の思考ストリームの記録**

でしょう。

この違いは結構大きい。

たとえば、

> 「あー今の面白い。というかここ前に試したやつとの差を説明したいな。あとこの前3秒ぐらい残して」

みたいな雑な発話を、

```json
{
  "anchor": 124.2,
  "highlight": true,
  "context_before": 3,
  "intent": "explain",
  "note": "以前試した構成との差を説明"
}
```

までLLMが勝手に構造化するところまでは、既存レビュー製品はあまり踏み込んでいない。

だから私は、

**SceneNoteをUXプロトタイプとして使う → 不満が見えたら mpv + Whisper のローカル版を作る**

がかなり良い順序だと思うわ。

そしてSceneNoteを見つけたことで、さっきの案が妄想ではなくなったのが面白い。**「voice note → timestamp → transcription → editor marker」までは既に成立している市場**なのよ。あとはそこへLLMによる意味構造化を挟めば、欲しいものにかなり近づく。

---

# you asked

message time: 2026-08-14 13:48:52

ymmに統合できないかな

---

# chatgpt response

できる可能性はかなり高いと思うわ。ポイントは **YMM4の中に全部作り込むか、外部の「音声アノテーション常駐ツール」とYMM4をつなぐか** で、私は後者から始めるのを推す。まず現行のプラグイン/API周りを確認するわ。

うん、**YMM4に直接統合するの、かなりアリ**。しかも調べたら、今のYMM4は `IToolPlugin` でWPFのツールパネルを生やせるし、既に別プロジェクトで「現在位置取得→VoiceItem追加」までやってる実例がある。なので「夢物語」ではなく、普通にプロトタイプできる段階よ。

## 事実

**できるし、YMM4の中に入れた方が今回の用途にはかなり自然**だと思うわ。

現行YMM4には `IToolPlugin` があり、WPFの独自ツールパネルをYMM4内に追加できる。公式にもプラグイン開発手順とサンプルが用意されていて、現在は .NET 10 / Windows向けで開発する形になっている。([YMM API Docs](https://ymm-api-docs.vercel.app/reference/yukkuri-movie-maker/plugin/i-tool-plugin?utm_source=chatgpt.com))

しかも面白い先行例があって、非公式の **YMM4 MCPプラグイン** はすでに、

- 現在のプレビュー位置取得
- 指定位置へのシーク
- 再生・停止
- `VoiceItem` の追加
- 複数セリフの一括追加
- タイムラインアイテム取得・編集

まで実装している。つまり、必要な部品は実際に動いているわ。([GitHub](https://github.com/SCPgamerscp/ymm4MCP))

ただし注意点もある。そのMCPプラグインのタイムライン操作の一部はYMM4内部APIを利用している。YMM4側も、公開 `YukkuriMovieMaker.Plugin.dll` のAPIだけに依存する方が本体更新に強く、内部実装への依存はメンテナンス性が落ちるという方針を明示している。([GitHub](https://github.com/manju-summoner/YukkuriMovieMaker.Plugin.Community))

## 推測（示唆）

なら私は、**「Voice Annotation for YMM4」みたいなツールプラグイン**にしてしまう。

操作はこれくらいまで削れると思う。

```text
YMM4で普通に動画再生
──────────────────────────────

                     🎙 PTT押す
01:24.38             ↓

「今のところ三秒前から残す。
 ここは爆発異常について説明」

                     ↓ PTT離す

そのまま動画再生継続
```

横のツールパネルだけ、

```text
Voice Annotations

01:24.38  💬
今のところ三秒前から残す。
ここは爆発異常について説明

02:08.13  ★
今の事故面白いから残す

03:41.20  ✂
ここ全部いらない
```

と増えていく。

### 内部構造もかなり単純にできる

```text
YMM4
 │
 ├─ Preview / Timeline
 │       │
 │       └─ current frame
 │
 └─ Voice Annotation Plugin
          │
          ├─ PTT
          ├─ Mic recording
          ├─ Whisper
          │
          └─ annotation.jsonl
```

PTTを押した瞬間、

```json
{
  "frame": 2531,
  "time": 84.367,
  "audio": "annotations/0017.wav"
}
```

を確保。

離したらWhisperを走らせて、

```json
{
  "frame": 2531,
  "time": 84.367,
  "audio": "annotations/0017.wav",
  "text": "今のところ三秒前から残す。ここは爆発異常について説明"
}
```

にする。

**録音中もYMM4の動画は止めない。**

ここが製品体験として一番大事ね。

---

### そしてYMM統合の最大のメリット

mpv版なら最終的に、

```text
mpv
→ annotation
→ script
→ YMM4へインポート
```

になる。

YMMプラグインなら、

```text
annotation
      ↓
「セリフ化」
      ↓
YMM4 VoiceItem
```

までその場でできる。

既存のYMM4 MCP実装では実際に `voice/add` と `script/add` 相当の操作が成立しているので、この方向にはかなり具体的な先例がある。([GitHub](https://github.com/SCPgamerscp/ymm4MCP))

たとえば一周見終わったあと、

```text
14 annotations

[ AIで整理 ]

01:24  explain
「ここでは爆発異常が……」
                 [セリフ化]

02:08  highlight
「今の挙動を見ると……」
                 [セリフ化]

03:41  cut
「不要区間」
                 [カット候補]
```

となる。

`セリフ化` を押したら、

```text
VoiceItem
├─ Text
├─ Character
├─ Voice
└─ Frame
```

としてYMMタイムラインへ入れる。

VOICEVOXなどはYMM4から直接音声生成できるので、そこまで入れば**音声＋字幕生成は既存YMMの仕事に戻せる**。([饅頭遣いのおもちゃ箱](https://manjubox.net/ymm4/faq/%E3%82%86%E3%81%A3%E3%81%8F%E3%82%8A%E3%83%9C%E3%82%A4%E3%82%B9/VOICEVOX%E3%82%92%E4%BD%BF%E7%94%A8%E3%81%99%E3%82%8B/?utm_source=chatgpt.com))

これがすごく綺麗。

---

### さらに「音声→操作」にしてしまう

ここはLLMを挟むともっと面白い。

入力：

> 「今の三秒前から残して、ここはインカーノン変形について魔理沙に説明させる」

解析：

```json
{
  "action": "narration",
  "anchor": {
    "relative": -3.0
  },
  "speaker": "魔理沙",
  "intent": "インカーノン変形を説明"
}
```

そうすると後処理で、

```text
84.3s marker
 ↓ -3s
81.3s

↓ AI台本

「ここでインカーノン形態へ移行する。
通常射撃とは挙動が大きく変わるぜ」

↓ YMM

VoiceItem @ 81.3s
```

まで行ける。

つまり最終的には**音声アノテーションが、そのままYMM4に対する自然言語編集コマンドになる**。

これは結構いいと思う。

---

### 私なら3段階で作る

**v0.1** は絶対に欲張らない。

```text
PTT
→ timestamp
→ WAV
→ Whisper
→ YMM内パネルに一覧表示
→ クリックでその位置へジャンプ
```

これだけ。

次に **v0.2**：

```text
自然言語
→ highlight / explain / cut / check
→ ±秒オフセット解釈
```

最後に **v0.3**：

```text
annotation
→ 台本生成
→ VoiceItem追加
→ VOICEVOX
→ 字幕
```

にする。

私は**最初からカット編集まで自動化しない方がいい**と思う。VoiceItem追加は失敗しても戻せるけど、タイムラインをリップル削除し始めるとUndoやアイテム整合性まで責任範囲が急激に増えるからね。

むしろ第一号としては、

> **YMM4を再生しながらマウスサイドボタンを押して好き勝手喋ると、タイムコード付きの文字メモが横に溜まる**

これだけでかなり使える。

しかも今調べた限り、**YMM4のプラグイン構造とものすごく相性がいい**。mpvで外から組むより、ここまで用途がYMM中心なら最初からYMM4プラグインにしたくなってきたわ。

---

# you asked

message time: 2026-08-14 13:56:19

https://github.com/annenpolka/takegraph にまとめたいんだけど、どんな形がいいと思う？

---

# chatgpt response

見た。今の TakeGraph はすでに「**YMM4は投影先、TakeGraph側が意味と履歴の正本**」という境界がかなり強くできてる。  
だから音声アノテーションを足すなら、YMM4専用機能として埋めるより、**TakeGraphの“入力チャネル”として一級概念にする**のが綺麗だと思うわ。

かなり相性いいわ。今の TakeGraph を見る限り、**「YMM4に音声アノテーション機能を足す」ではなく、TakeGraph全体を `capture → understand → propose → commit` の流れに拡張する**のが一番綺麗だと思う。

## 事実

TakeGraph はすでに「script が canonical」「generated voice は immutable take」「AI編集は reviewable patch」という思想で組まれている。READMEでもその3つがプロダクトの核として明示されているわ。

アーキテクチャ上も、YMM4は正本ではなく projection target で、意味・revision・patch は TakeGraph 側が持つ設計になっている。 また、YMM4のscene observationは project / scene / fingerprint / fps にsource-boundされ、分からないものを推測しない方針になっている。

既存の音声モデルも、

```rust
VoiceTake
├─ immutable
├─ input_hash
├─ query_hash
├─ AudioArtifact
└─ Accepted / Rejected / Stale ...
```

という生成音声用のimmutable lifecycleになっている。

なので、今回の「人間が動画を見ながら喋った声」は **`VoiceTake` には入れない方がいい**。役割が全く違うのよ。

---

## 推測（示唆）

私は TakeGraph をこう拡張する。

```text
                    HUMAN
                      │
                      │ watching + speaking
                      ▼
              ┌───────────────┐
              │ Capture       │
              │ Annotation    │
              └───────┬───────┘
                      │
                      ▼
              ┌───────────────┐
              │ Interpretation│
              │ ASR + Agent   │
              └───────┬───────┘
                      │
          ┌───────────┼───────────┐
          ▼           ▼           ▼
       highlight   narration     cut
                       │
                       ▼
                ManagedCueIntent
                       │
                       ▼
                    Patch
                       │
                 approve/review
                       │
                       ▼
                     YMM4
```

つまり **Annotationを「編集前の一次資料」として一級概念にする**。

これが TakeGraph という名前にも妙に合う。

---

# 1. `Annotation` は immutable evidence にする

ここ、かなり強く推したい。

例えば：

```rust
pub struct Annotation {
    pub id: AnnotationId,
    pub session_id: CaptureSessionId,

    pub anchor: SourceAnchor,

    pub audio: AudioArtifactRef,
    pub captured_at: Timestamp,

    pub context_before_ms: u32,
    pub context_after_ms: u32,
}
```

そしてanchorは単に、

```rust
frame: 2531
```

じゃ弱い。

こうする。

```rust
pub struct SourceAnchor {
    pub project_id: String,
    pub scene_id: String,
    pub project_fingerprint: String,
    pub fps: Rational,
    pub preview_frame: i64,
}
```

既存のTakeGraphはすでにscene observationをsource-boundにしているので、この思想をそのまま流用できる。

これなら、

> 01:24の「今のところ」

が**どの状態のYMM4を見ていたときの01:24なのか**

まで残る。

単純なtimestampメモより一段強いわ。

---

# 2. transcriptはAnnotationそのものにしない

これも重要。

```text
Annotation
   │
   ├─ raw audio
   │
   └─ Transcript
       ├─ model
       ├─ model_version
       ├─ text
       └─ confidence
```

くらいに分離する。

例えば：

```rust
pub struct TranscriptArtifact {
    pub annotation_id: AnnotationId,
    pub text: String,
    pub engine: String,
    pub engine_version: String,
    pub audio_hash: String,
}
```

理由は、

> ASR結果は後で変えられる

から。

最初はWhisper smallで、

```text
「爆発異常」
```

を妙な文字にしたとしても、後日モデルを変えて再認識できる。

**raw voice が evidence、transcript は derived artifact**。

TakeGraphのcontent-addressed artifact思想とかなり噛み合うと思う。

---

# 3. さらに `Interpretation` を分ける

ユーザーが喋る内容を型に押し込まない。

例えば普通に、

> 「あー今の残したいな。あと三秒くらい前から。ここCompressionの説明入れる」

と喋る。

raw transcript：

```text
あー今の残したいな。
あと三秒くらい前から。
ここCompressionの説明入れる。
```

Agent interpretation：

```json
{
  "annotationId": "ann-31",
  "referent": {
    "relativeStartMs": -3000
  },
  "intents": [
    {
      "kind": "keep"
    },
    {
      "kind": "narration",
      "topic": "Primary Compression"
    }
  ]
}
```

つまり、

```text
Audio
 ↓
Transcript
 ↓
Interpretation
```

をそれぞれ別ノードにする。

ここ、本当に **TakeGraph** になってくる。

---

# 4. Annotationを直接Timeline Editにしない

ここが一番重要かもしれない。

例えば、

> 「ここカット」

と言ったからといって、

```text
Annotation
    ↓
YMM4 Delete
```

にはしない。

必ず、

```text
Annotation
 ↓
Interpretation
 ↓
Edit Proposal
 ↓
Patch
 ↓
Review
 ↓
Commit
```

にする。

今のTakeGraphの「AI編集はreviewable patch」という思想をそのまま守れる。READMEでもMCP側はstage → approve → executeというtask envelopeに寄せている。

**音声入力はコマンドではなく、intent acquisition。**

この区別はかなり大事よ。

---

# 5. Captureは canonical revision を進めない

これは設計上かなり重要な提案。

動画を見ながら、

```text
annotation
annotation
annotation
annotation
```

と20個喋っただけでTakeGraph revisionが20進むようにはしない。

なぜなら現在のTakeGraphでは、patch approvalやstale baseがcanonical revisionに強く結びついている。

アノテーションするたびにrevisionを進めたら、

```text
Patch A Approved @ Rev 15

↓

「ここ面白い」
Annotation

↓

Rev 16

↓

Patch A stale
```

という、なかなか味わい深い自爆装置になる。

だから、

```text
Project Revision
     │
     └──── capture session

CaptureSession Revision
      1
      2
      3
      4
```

という**別系列**にする。

Annotationは、

```rust
observed_project_revision: RevisionId
```

を持つだけ。

そして編集へpromoteするとき初めて、

```text
annotation
 ↓
stage patch against current head
 ↓
stale check
 ↓
canonical revision
```

に乗せる。

これはかなり綺麗でしょう。

---

# 6. YMM4 bridgeは薄く保つ

今のrepo構造なら、

```text
bridges/ymm4/
```

には、

```text
🎙 PTT
↓
current project
current scene
current frame
fingerprint
↓
capture request
```

くらいまでしか持たせない方がいい。

ASRもLLMもC#側へ入れない。

概念的には、

```text
YMM4
 │
 │ PTT down
 ▼
TakeGraph.Ymm4Bridge
 │
 ├─ preview frame
 ├─ project identity
 └─ mic capture
        │
        ▼
takegraph-node
        │
        ├─ audio CAS
        ├─ ASR
        └─ annotation service
```

ね。

既存architectureでも provider / platform I/O は `takegraph-node` が持つ境界になっている。

だからここにも合わせる。

---

# 7. repo構造ならこう

私は最終的にこうしたい。

```text
crates/
  takegraph-core/
    src/
      annotation.rs
      capture.rs

  takegraph-service/
    src/
      annotation_store.rs
      annotation_interpretation.rs
      annotation_promotion.rs

  takegraph-node/
    src/
      transcription/
        mod.rs
        whisper.rs
      audio_capture/
        mod.rs

bridges/
  ymm4/
    TakeGraph.Ymm4Bridge/
      Capture/
        VoiceAnnotationTool.cs
        CaptureSession.cs

apps/
  studio-view/
    src/
      features/
        annotations/
          AnnotationList.tsx
          AnnotationReview.tsx

specs/
  protocols/
    annotation_promotion.qnt
```

ただ、`capture.rs` をcoreまで持ってくるかは少し迷う。

OS microphone capture自体は当然nodeだけど、

```text
CaptureSession
Annotation
SourceAnchor
Interpretation
```

という**純粋な意味モデル**はcoreでいいと思う。

---

# 8. MCP surfaceは増やさない

これも現在の設計を見ると重要。

READMEではモデル向けMCP surfaceを意図的に、

```text
takegraph_inspect
takegraph_task_stage
takegraph_task_approve
takegraph_task_execute
takegraph_task_decide
```

の5つに絞っている。

だったら、

```text
takegraph_annotation_create
takegraph_annotation_transcribe
takegraph_annotation_promote
...
```

なんて増やさない方がいい。

例えば、

```json
takegraph_inspect({
  "view": "annotations"
})
```

と、

```json
takegraph_task_stage({
  "kind": "annotation_promotion",
  "annotationIds": ["ann-1", "ann-2"]
})
```

程度でいい。

PTTなどのgestureはREADMEの方針通り、**app-only / bridge-only** に置けばいい。

これは既存設計とかなり綺麗に揃う。

---

## さらに先：Narrative Graph

ここから一段飛躍するなら、最終的には私は **AnnotationをScriptへ直結させない**。

間に、

```text
NarrativeIntent
```

を入れたい。

例えば、

```text
Annotation #31
「ここCompressionの説明」

      ↓

NarrativeIntent
topic: Primary Compression
purpose: explain
anchor: combat-clip-17

      ↓

ScriptCandidate A
「ここで重要なのがPrimary Compressionで……」

ScriptCandidate B
「この異常の入り方を見ると……」

      ↓

ManagedCueIntent
```

今の `ManagedCueIntent` はすでに、

- display_text
- spoken_text
- speaker_role
- voice_profile
- placement
- realization preference

をYMM4非依存で持っている。

だから、

```text
Human Annotation
       ↓
Narrative Intent
       ↓
ManagedCueIntent
       ↓
VoiceTake
       ↓
YMM4 realization
```

が非常に自然。

私はこれが **TakeGraphの本当の中核**になりうると思う。

---

## 最初の実装スライス

全部一気に作る必要はないわ。

第一号はこれだけでいい。

```text
YMM4再生
   ↓
PTT
   ↓
WAV + current frame + source fingerprint
   ↓
Annotation保存
   ↓
Whisper
   ↓
YMM4 Tool Panelに

01:24
「今のところ三秒前から残す。
ここはCompressionの説明」

[▶ jump] [dismiss]
```

**AIによる分類すら後回し。**

その次に、

```text
[Interpret]
```

を足して、

```text
keep
narration: Primary Compression
offset: -3 sec
```

を出す。

最後に、

```text
[Promote to edit]
```

で既存のPatch/Task系へ流す。

これなら既存TakeGraphをほとんど壊さない。

---

## 私なら概念をこう整理する

```text
Observation
    ↓
Annotation        人間が気付いたこと
    ↓
Interpretation    AIが意味を構造化したもの
    ↓
Intent            動画で何をしたいか
    ↓
Patch             具体的な変更案
    ↓
Realization       YMM4上でどう実現するか
```

この6層。

**特に `Annotation ≠ Intent ≠ Patch` を守る。**

ここをまとめて「音声コマンド」にすると、最初は楽だけど後から絶対に苦しくなると思う。逆にこの三つを分ければ、音声以外にも、

```text
voice annotation
text memo
game event
automatic VLM observation
ASR transcript
viewer comment
```

を全部同じ入口へ持ってこられる。

するとTakeGraphは「AI動画編集ソフト」からもう少し抽象度が上がって、

> **人間や機械が観測した素材から、編集意図をグラフとして育て、それをreviewable patchとしてNLEへ実現するシステム**

になる。

今のrepoがすでに **agent-native / voice-first / reviewable patches** を掲げていることを考えると、かなり自然な進化だと思うわ。

次に手を付けるなら、私はまず **`docs/voice-annotation.md` でこの境界を固定してから `annotation.rs` に落とす**。実装より先に `Annotation / Interpretation / Intent / Patch` の違いだけ文章で殺し切っておく価値がある。ここが曖昧だと、後で全部が「なんかAIが出したJSON」になるからね。

---

# you asked

message time: 2026-08-14 14:00:25

やることを詳しく詰めて

---

# chatgpt response

今のTakeGraphの境界を崩さず、ドメイン・保存・YMM4/ASR/MCP/UI・テストまで実装単位へ分解するわ。まず既存task surfaceとbridgeを確認し、差し込み位置と依存順、受け入れ条件を固める。

---

# chatgpt response

YMM4 bridgeには既に`IToolPlugin`のビュー枠があるので録音UIに流用できるわ。録音データはYMM4内部へ持たせずTakeGraphへ渡すAPI境界を次に確定する。

---

# chatgpt response

さらに`current_scene_composition()`でproject/scene/fingerprint/fps/frameを検証付き取得でき、アンカー契約は再利用できるわ。新デーモンは増やさず、bridgeは即時録音、正本化・解釈はTakeGraph側へ寄せる。

---

# chatgpt response

そうね。現状の TakeGraph を踏まえると、これは単に「YMM4に録音ボタンを生やす」話ではないわ。  
**人間の気付きが、編集パッチへ昇格するまでの新しい入力経路**として設計するべきね。

私なら最終形をこう定義する。

```text
YMM4で動画を見る
  ↓
ホットキーを押して雑に喋る
  ↓
時刻付きの音声アノテーションとして保存
  ↓
文字起こし
  ↓
AIが意図・対象区間を構造化
  ↓
人間が修正・採用
  ↓
既存の timeline_edit としてstage
  ↓
digest付きでreview / execute
```

重要なのは、**音声アノテーションを編集コマンドにしないこと**よ。

---

# 事実

TakeGraph は現在、

- script が canonical
- 生成音声は immutable take
- AI編集は reviewable patch
- YMM4は canonical database ではなく projection target

という境界で作られている。 

現在の `VoiceTake` は生成音声の候補・採用・却下・staleを扱うモデルであり、入力された人間の音声メモとは役割が違う。したがって、音声アノテーションを `VoiceTake` へ押し込むべきではないわ。

一方、YMM4の現在位置については、Rust側にすでに検証済みの `current_scene_composition()` がある。これは、

```text
projectId
sceneId
sourceFingerprint
fps
frame
```

を取得し、schema・source identity・並び順・利用不能値まで検証する読み取り専用契約になっている。再生位置を動かさず、YMM4プロジェクトも変更しない。 

つまり、アノテーションのタイムスタンプ取得は新規にYMM4内部APIを掘る必要がない。

また、YMM4プラグインにはすでに `IToolPlugin` のビューがあり、現在はbridgeの状態表示程度しかしていない。ここはアノテーション操作パネルに拡張できる。 

ただし、現行のインストーラーはbridgeの単一DLLをハッシュ検証しながら置換する構造よ。プラグインへ音声ライブラリを直接追加すると、依存DLL一式のインストール・バックアップ・復旧まで作り直す必要がある。 

---

# 推測（示唆）

## まず決めるべき構成

私は次の分離を薦める。

```text
┌────────────────────────────┐
│ YMM4 Tool Panel            │
│ ・録音状態表示              │
│ ・開始 / 停止               │
│ ・最近のアノテーション一覧  │
│ ・クリックでシーク          │
└─────────────┬──────────────┘
              │ local IPC
              ▼
┌────────────────────────────┐
│ TakeGraph Capture Host     │
│ ・マイク入力                │
│ ・ホットキー                │
│ ・YMM4 current frame取得    │
│ ・WAV/CAS保存               │
│ ・annotation store更新      │
└─────────────┬──────────────┘
              │
              ▼
┌────────────────────────────┐
│ Transcription / Agent      │
│ ・Whisper等                 │
│ ・意図分類                  │
│ ・台本候補                  │
└─────────────┬──────────────┘
              │
              ▼
┌────────────────────────────┐
│ Existing TakeGraph Tasks   │
│ timeline_edit              │
│ stage → execute            │
└────────────────────────────┘
```

### マイク録音をYMM4 bridgeへ直接入れない

ここはかなり強く言っておくわ。

bridgeにNAudioなどを直接入れると、

- bridgeの責務がYMM4 adapterからWindows音声入力へ膨らむ
- 単一DLLインストーラーが崩れる
- 音声デバイス障害がbridge全体を巻き込む
- 将来mpvや別NLEから同じ機能を使えない

という問題が出る。

だから、**YMM4 bridgeは「現在位置を教える」「パネルを表示する」だけ**。  
録音は別プロセスの Capture Host にする。

YMM4との統合感は保てるけれど、実装上は疎結合になる。こういう嘘のない統合の方が長持ちするわ。

---

# 完成時の操作感

最初に目指すUXはこれで十分。

```text
YMM4で動画を再生中

F8
  → 録音開始
  → 右下に「REC 01:24.38」

「今のところ三秒前から残す。
 ここはPrimary Compressionの説明を入れる」

F8
  → 録音終了
  → 動画は止まらない

数秒後：

01:24.38
[解説候補]
今のところ三秒前から残す。
ここはPrimary Compressionの説明を入れる。

[ジャンプ] [台本化] [破棄]
```

最初は**押しっぱなしのPTTではなく、開始・停止のトグル式**を薦める。

Windowsのグローバルホットキーで「キーを離した瞬間」まで安全に取るのは、低レベルキーボードフックやRaw Inputが必要になり、フォーカス移動やキー取りこぼしを処理する羽目になる。第一号でそこへ行くのは勇敢というより、単に沼へ正装して入る行為よ。

トグル式が動いた後に、

- hold-to-talk
- マウスサイドボタン
- フットペダル
- ゲームパッド

を足せばいい。

---

# 1. ドメインモデルを追加する

新規ファイル：

```text
crates/takegraph-core/src/annotation.rs
```

最低限、次を定義する。

```rust
pub struct AnnotationCapture {
    pub id: AnnotationId,
    pub session_id: CaptureSessionId,

    pub start_anchor: SourceAnchor,
    pub end_anchor: SourceAnchor,

    pub audio: CapturedAudioEvidence,
    pub captured_at_utc: String,

    pub stability: CaptureStability,
}
```

アンカー：

```rust
pub struct SourceAnchor {
    pub project_id: String,
    pub scene_id: String,
    pub source_fingerprint: String,

    pub fps: u32,
    pub frame: i32,

    pub observed_canonical_revision: Option<RevisionId>,
}
```

音声証拠：

```rust
pub struct CapturedAudioEvidence {
    pub audio_sha256: String,
    pub byte_length: u64,

    pub duration_samples: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}
```

状態：

```rust
pub enum CaptureStability {
    Stable,
    SourceChanged,
}
```

## なぜ開始と終了のアンカーを両方持つのか

動画を止めずに喋るなら、

```text
録音開始：01:24
録音終了：01:31
```

になる。

その7秒間に、

- 再生位置だけ進んだ
- シーンが変わった
- プロジェクトが切り替わった
- タイムラインが編集された

のどれかが起こりうる。

開始時と終了時の、

```text
projectId
sceneId
fingerprint
fps
frame
```

を比較すれば、

```text
同じsourceのまま再生だけ進んだ
```

のか、

```text
アノテーション中に対象そのものが変わった
```

のかを判定できる。

`sourceFingerprint` が違った場合も音声は捨てない。ただし `SourceChanged` として、後続の自動配置を禁止する。

---

# 2. CaptureとTranscriptを分離する

文字起こしは `AnnotationCapture` のフィールドに直接入れない。

```rust
pub struct AnnotationTranscript {
    pub id: TranscriptId,
    pub capture_id: AnnotationId,

    pub audio_sha256: String,

    pub text: String,
    pub segments: Vec<TranscriptSegment>,

    pub provider_id: String,
    pub provider_digest: String,

    pub transcript_digest: String,
}
```

理由は単純。

```text
raw audio
```

は観測証拠だけれど、

```text
Whisperが出した文字列
```

は再生成可能な派生物よ。

モデルや辞書を変えたら、同じ音声から別の文字列が出る。

したがって、

```text
AnnotationCapture
    └─ Transcript revision 1
    └─ Transcript revision 2
    └─ Human-corrected revision 3
```

にする。

**修正時も上書きしない。**

TakeGraphはすでに音声アーティファクトをcontent-addressedに保存し、同じ内容は再利用しつつ異なる内容を上書きしない設計を持っている。この流儀をそのまま使える。

---

# 3. Annotation Storeをcanonical project storeから分ける

新規：

```text
crates/takegraph-service/src/annotation_store.rs
```

私は `DurableProjectState` に直接突っ込まない。

現在のproject storeは、canonical revision・target link・external commit・verified receiptを管理するappend-only storeになっている。

アノテーションを喋るたびにcanonical revisionを進めると、

```text
編集Patchをstage
↓
音声メモを1つ追加
↓
base revisionが古くなる
↓
Patch stale
```

というコントが始まる。

だから別系列にする。

```text
Canonical Project Store
  head: revision 42

Annotation Store
  generation: 128
```

アノテーション追加・文字起こし・分類では、canonical revisionを変えない。

## Annotation Storeのイベント

```rust
pub enum AnnotationEvent {
    CaptureImported {
        capture: AnnotationCapture,
    },

    TranscriptAttached {
        capture_id: AnnotationId,
        transcript: AnnotationTranscript,
    },

    InterpretationAttached {
        capture_id: AnnotationId,
        interpretation: AnnotationInterpretation,
    },

    Dismissed {
        capture_id: AnnotationId,
        reason: Option<String>,
    },

    PromotionStaged {
        capture_id: AnnotationId,
        interpretation_digest: String,
        task_id: String,
        plan_digest: String,
        base_revision: RevisionId,
    },

    PromotionCommitted {
        capture_id: AnnotationId,
        task_id: String,
        committed_revision: RevisionId,
        receipt_digest: String,
    },
}
```

これをappend-only・hash-chainで保存する。

同じ `capture_id` の再送は、

- 同じaudio hashならidempotent replay
- 違うaudio hashならconflict

にする。

---

# 4. Capture Hostを作る

新規crate：

```text
crates/takegraph-capture/
```

最初は独立GUIアプリにしなくていい。

```text
takegraph-cli annotation listen
```

という長寿命サブコマンドから始める。

構成：

```text
crates/takegraph-capture/
  src/
    lib.rs
    audio_input.rs
    capture_session.rs
    hotkey.rs
    local_server.rs
    wav.rs
```

## 責務

Capture Hostが持つもの：

- マイクデバイス列挙
- 録音開始・停止
- WAVの一時保存
- SHA-256計算
- YMM4の現在位置取得
- Annotation Storeへの登録
- ASRジョブのenqueue
- ローカルIPC
- グローバルホットキー

持たないもの：

- LLMによる意味解釈
- YMM4タイムライン変更
- VOICEVOX生成
- Patch approval
- canonical revision変更

## 音声形式

第一号は固定でいい。

```text
PCM WAV
16 kHz
mono
16-bit
```

高音質な録音作品ではない。文字起こしと聞き返し用よ。

入力デバイスのネイティブ形式が異なる場合だけCapture Host内で変換する。

## 音声ライブラリ

Rust側なら、

```text
cpal
+
WAV writer
```

程度でいい。

Windows依存のホットキーは `windows` crateで分離する。

```rust
trait AudioCaptureBackend {
    fn devices(&self) -> Result<Vec<AudioDevice>>;
    fn start(&mut self, device: &AudioDevice) -> Result<CaptureHandle>;
    fn stop(&mut self, handle: CaptureHandle) -> Result<CapturedPcm>;
}
```

こうしておけば、録音バックエンドを替えてもdomain/serviceは変わらない。

---

# 5. Capture Hostの状態機械

これは明示しておいた方がいい。

```text
Idle
  ↓ Start
Starting
  ↓ mic opened + start anchor acquired
Recording
  ↓ Stop
Finalizing
  ↓ WAV/CAS/store committed
Captured
  ↓ ASR
Transcribing
  ↓
Ready

どこかで失敗
  ↓
Failed
```

具体的には：

```rust
pub enum CaptureHostState {
    Idle,

    Starting {
        capture_id: AnnotationId,
    },

    Recording {
        capture_id: AnnotationId,
        start_anchor: SourceAnchor,
    },

    Finalizing {
        capture_id: AnnotationId,
    },

    Failed {
        capture_id: Option<AnnotationId>,
        message: String,
    },
}
```

## 開始処理

```text
1. YMM4 current_scene_composition()取得
2. source anchor検証
3. capture ID予約
4. 一時WAV作成
5. マイク開始
6. Recordingへ遷移
```

マイク開始に失敗したら、Annotationを作らない。

## 停止処理

```text
1. マイク停止
2. WAV flush
3. 終了時のcurrent_scene_composition()取得
4. WAV metadata解析
5. SHA-256計算
6. CASへatomic publish
7. AnnotationCaptureをappend
8. ASR enqueue
9. UI通知
```

大事なのは、

```text
WAV保存成功
↓
Annotation Store更新失敗
```

のような中間状態を復旧できるようにすること。

一時ファイルは、

```text
*.partial
```

としておき、成功時だけCAS名へrenameする。

Capture Host再起動時には、

- valid WAVならrecovery候補
- 不完全WAVならquarantine
- storeに既存captureがあればreplay

とする。

---

# 6. Capture HostとYMM4 panelの通信

既存bridgeと同じく、loopback＋ランダムトークンでいい。

```text
%LOCALAPPDATA%\TakeGraph\capture-host.json
```

```json
{
  "endpoint": "http://127.0.0.1:8767",
  "token": "random-secret"
}
```

APIは小さくする。

```text
GET  /v1/health
GET  /v1/status
GET  /v1/devices
GET  /v1/annotations?limit=20

POST /v1/capture/start
POST /v1/capture/stop
POST /v1/capture/cancel
POST /v1/config/device
```

ここで重要なのは、

> **MCPツールから録音を開始できないようにする**

こと。

録音開始は、

- YMM4のローカルUI
- ローカルホットキー

からしか呼ばせない。

モデルにマイク起動権を渡す必要はないわ。そんな機能は便利になる前に怪談になる。

---

# 7. YMM4 Tool Panelを作り替える

現在の `TakeGraphBridgeView` は状態表示だけなので、ここをCapture Hostのコントロールパネルにする。

ファイル構成はこうする。

```text
bridges/ymm4/TakeGraph.Ymm4Bridge/
  TakeGraphBridgeView.xaml
  TakeGraphBridgeView.xaml.cs
  TakeGraphBridgeViewModel.cs

  Annotation/
    CaptureHostClient.cs
    AnnotationRowViewModel.cs
    AnnotationPanelState.cs
```

コードだけでWPF UIを組み続けると、ここから先は見事な木工細工になる。XAMLへ移した方がいい。

## パネル内容

```text
TakeGraph Voice Notes

Capture Host: ● Connected
Microphone: [USB Microphone ▼]
Hotkey: F8

[● Start recording]

REC 00:04.2
01:24.38 → 01:28.57

Recent notes
────────────────────
01:24.38  文字起こし中…
[Jump] [Cancel]

00:52.10  今の事故は残す
[Jump] [Review] [Dismiss]
```

## パネルが持つ操作

- Capture Host接続確認
- 録音開始・停止
- 録音状態表示
- 最近のAnnotation取得
- 現在位置へのジャンプ
- dismiss
- Studio viewを開く

## シーク

第一号では、YMM4 panel内からin-processでシークしていい。

ただし、将来Studio MCP Appからもジャンプしたくなるので、最終的にはbridgeに読み取り寄りのtransient controlを足す。

```text
POST /v2/scene/seek
```

入力：

```json
{
  "projectId": "...",
  "sceneId": "...",
  "expectedFingerprint": "...",
  "frame": 2531
}
```

出力：

```json
{
  "requestedFrame": 2531,
  "actualFrame": 2531,
  "sourceFingerprint": "...",
  "projectDirtyStateChanged": false
}
```

これはcanonical editではないのでPatchにする必要はない。ただし、別プロジェクトへ誤シークしないようsource-boundにはする。

---

# 8. 文字起こしproviderを追加する

新規：

```text
crates/takegraph-node/src/transcription.rs
crates/takegraph-node/src/whisper_cpp.rs
```

インターフェース：

```rust
#[async_trait]
pub trait TranscriptionProvider {
    async fn transcribe(
        &self,
        audio: &CapturedAudioEvidence,
        audio_path: &Path,
    ) -> Result<TranscriptResult, TranscriptionError>;

    fn provider_digest(&self) -> String;
}
```

第一backendは外部のwhisper.cpp等を呼ぶ方式でいい。

```text
TakeGraphがモデルをbundledする
```

のではなく、

```text
ユーザー管理のASR executable / modelを呼ぶ
```

形にする。

VOICEVOXをユーザー管理のloopback providerとして扱っている現在の設計とも揃う。

記録するprovider情報：

```json
{
  "provider": "whisper-cpp",
  "executableSha256": "...",
  "modelSha256": "...",
  "language": "ja",
  "parameters": {
    "temperature": 0
  }
}
```

モデル名だけでは弱い。  
**同じaudioから何を使って文字列を生成したか**をdigestに含める。

## ASR失敗時

```text
Capture = valid
Transcript = failed
```

にする。

絶対に、

```text
ASR失敗
↓
音声アノテーション全体を失敗扱い
```

にはしない。

人間の生音声が残っていれば、後から別ASRで再処理できる。

---

# 9. Interpretationを追加する

次の段階で、自由発話を構造化する。

```rust
pub struct AnnotationInterpretation {
    pub id: InterpretationId,
    pub capture_id: AnnotationId,
    pub transcript_digest: String,

    pub temporal_reference: TemporalReference,
    pub intents: Vec<AnnotationIntent>,

    pub model_id: String,
    pub model_digest: String,
    pub interpretation_digest: String,
}
```

意図：

```rust
pub enum AnnotationIntent {
    Note,

    Highlight {
        reason: Option<String>,
    },

    Narration {
        topic: String,
        draft_hint: Option<String>,
    },

    CutCandidate {
        reason: Option<String>,
    },

    Verify {
        question: String,
    },
}
```

時間参照：

```rust
pub struct TemporalReference {
    pub reference_frame: i32,
    pub start_offset_frames: i32,
    pub end_offset_frames: Option<i32>,
    pub relation: TemporalRelation,
}
```

```rust
pub enum TemporalRelation {
    Before,
    At,
    After,
    Range,
}
```

たとえば、

> 今のところ三秒前から残す。ここはCompressionの説明を入れる

なら、

```json
{
  "referenceFrame": 2531,
  "startOffsetFrames": -180,
  "relation": "range",
  "intents": [
    {
      "type": "highlight",
      "reason": "残す"
    },
    {
      "type": "narration",
      "topic": "Primary Compression"
    }
  ]
}
```

になる。

ただし、**これはAIの候補**よ。

Capture evidenceでも、編集Patchでもない。

---

# 10. 台本候補へ変換する

`Narration` intentから、

```text
ScriptCandidate
```

を作る。

```rust
pub struct ScriptCandidate {
    pub id: ScriptCandidateId,
    pub source_interpretation_digest: String,

    pub display_text: String,
    pub spoken_text: Option<String>,
    pub speaker_role: Option<String>,

    pub placement: NarrationPlacement,
}
```

配置：

```rust
pub struct NarrationPlacement {
    pub anchor_frame: i32,
    pub relation: TemporalRelation,
    pub preferred_start_frame: i32,
    pub maximum_end_frame: Option<i32>,
}
```

既存の `ManagedCueIntent` は、

- display text
- spoken text
- speaker
- placement
- realization preference
- fallback

をYMM4非依存で保持しているので、最終的な台本候補はここへ落とせる。

流れはこう。

```text
AnnotationCapture
  ↓
Transcript
  ↓
Interpretation
  ↓
ScriptCandidate
  ↓ human review
ManagedCueIntent
  ↓
TimelineEditPlan
```

---

# 11. 既存の `timeline_edit` に昇格させる

私は最初から `annotation_promotion` という新しいMCP task kindを増やさない。

現在のモデル向けsurfaceは5ツールに意図的に絞られていて、task kind追加はfacade・task envelope・agent skill・テストの同期が必要になる。 

Narrationが確定したら、既存の、

```text
timeline_edit
```

としてstageすればいい。

ただしplanにprovenanceを追加する。

```rust
pub struct SourceEvidenceRef {
    pub annotation_id: AnnotationId,
    pub capture_audio_sha256: String,
    pub transcript_digest: String,
    pub interpretation_digest: String,
}
```

操作：

```json
{
  "op": "native_voice_create",
  "entityId": "annotation-31-narration-1",
  "displayText": "ここで重要なのがPrimary Compressionです。",
  "characterName": "ゆっくり霊夢",
  "frame": 2351,
  "layer": 2,
  "maxLength": 300,
  "sourceEvidence": {
    "annotationId": "ann-31",
    "transcriptDigest": "...",
    "interpretationDigest": "..."
  }
}
```

この `sourceEvidence` もplan digestに含める。

そうすれば、

```text
何を根拠にこの台詞が生まれたか
```

を後から辿れる。

## CutCandidateはまだ実行しない

現状の `timeline_edit` は、

```text
portable_voice_create
native_voice_create
```

までしかaggregate operationとして扱っていない。

したがって第一段階では、

```text
「ここカット」
```

はあくまで `CutCandidate` として保持する。

無理にdelete/rippleへ変換しない。

Timelineの削除・分割・リップルを正式に実装してから、別のreviewable operationへ昇格させるべきね。

---

# 12. Studio ViewへAnnotation Reviewを追加する

現在のStudio Viewはおおむね、

```text
script
preview
voice
```

という構成で、host bridgeにもannotation用のAPIはない。 

追加する領域：

```text
Annotations
```

ファイル：

```text
apps/studio-view/src/features/annotations/
  AnnotationList.tsx
  AnnotationDetail.tsx
  TranscriptEditor.tsx
  InterpretationEditor.tsx
  ScriptCandidatePanel.tsx
```

## 画面構成

```text
┌ Annotation List ────────────┐
│ 01:24 explain               │
│ 02:08 highlight             │
│ 03:31 cut candidate         │
└─────────────────────────────┘

┌ Detail ─────────────────────┐
│ Audio: [▶]                  │
│                             │
│ Transcript                  │
│ 「今のところ…」             │
│                             │
│ Interpretation              │
│ narration / -3.0 sec        │
│                             │
│ [Edit] [Interpret again]    │
└─────────────────────────────┘

┌ Promotion ──────────────────┐
│ Script candidate            │
│ 「ここで重要なのが…」       │
│                             │
│ Character: ゆっくり霊夢     │
│ Frame: 2351                 │
│                             │
│ [Stage timeline edit]       │
└─────────────────────────────┘
```

YMM4 panelは「収集」に特化する。  
Studio Viewは「整理・意味解釈・昇格」に特化する。

この分離がいい。

---

# 13. MCP surfaceの変更

## 初期段階

モデル向け5ツールは増やさない。

追加するのは `takegraph_inspect` のviewだけ。

```json
{
  "view": "annotations"
}
```

返すもの：

```text
annotation ID
frame
transcript summary
intent candidates
stale status
promotion status
```

返さないもの：

```text
ローカル音声ファイルパス
マイクデバイスID
capture-host token
ASR executable path
model path
```

現在のfacadeも内部pathやnative handleをmodel-facing detailsから除外する方針を持っているので、そのまま徹底する。

## App-only tools

UI用には別途、

```text
annotation_list
annotation_get
annotation_transcribe
annotation_interpret
annotation_update_transcript
annotation_dismiss
annotation_stage_narration
```

を用意していい。

ただしmodel-facingの一般ツール一覧には出さない。

`annotation_stage_narration` は内部で既存 `timeline_edit` taskを作り、その通常のTaskEnvelopeを返す。

---

# 14. Quintで仕様化する部分

録音デバイス自体をQuintでモデル化する必要はない。  
マイクを有限状態機械で神聖化しても、現実のドライバは普通に裏切るからね。

Quintで扱うべきなのは、**AnnotationからPatchへ昇格する境界**よ。

新規：

```text
specs/protocols/annotation_promotion_protocol.qnt
```

状態：

```text
Captured
Imported
Transcribed
Interpreted
Staged
Promoted
Dismissed
Stale
```

主要invariant：

```text
1. Captureのaudio hashは変化しない
2. Transcriptはexact audio hashへ束縛される
3. Interpretationはexact transcript digestへ束縛される
4. Promotionはexact interpretation digestへ束縛される
5. Capture/Transcript/Interpretationではcanonical revisionが進まない
6. source fingerprintがstaleなら自動promotionできない
7. 同一capture IDを異なるaudio hashでimportできない
8. 同一promotion taskのexecuteはat-most-once
9. canonical revisionはverified YMM4 read-back後にだけ進む
10. dismissed annotationを暗黙に再promoteできない
```

既存repoにはpatch・YMM4 apply・composition graphなどのQuint protocolがすでにあり、この追加も同じ場所へ収められる。

---

# 実装PRの切り方

## PR 1: 設計境界を固定する

追加：

```text
docs/voice-annotation.md
docs/architecture.md
```

決める内容：

- Capture / Transcript / Interpretation / Intent / Patchの違い
- canonical revisionを進めないこと
- 生音声の保存方針
- source anchor
- privacy boundary
- model-facing / app-only boundary
- stale判定
- unsaved projectの扱い

### 完了条件

ドキュメントを読めば、

```text
何がevidenceで
何がAIの推測で
何が編集命令か
```

が曖昧でない。

---

## PR 2: Annotation domainとstore

変更：

```text
crates/takegraph-core/src/annotation.rs
crates/takegraph-core/src/lib.rs

crates/takegraph-service/src/annotation_store.rs
crates/takegraph-service/src/lib.rs
```

実装：

- IDs
- SourceAnchor
- CapturedAudioEvidence
- AnnotationCapture
- AnnotationEvent
- append-only store
- canonical serialization
- hash chain
- replay/conflict
- store projection

### 完了条件

fixtureから、

```text
captureを登録
↓
再読込
↓
同じcaptureをreplay
↓
canonical project headは不変
```

をテストできる。

---

## PR 3: Capture Hostの最小縦切り

追加：

```text
crates/takegraph-capture/
crates/takegraph-cli/src/annotation.rs
```

実装：

- マイク列挙
- トグル録音
- current_scene_composition取得
- start/end anchor
- WAV保存
- CAS publish
- Annotation Store append
- CLI表示

操作：

```powershell
takegraph annotation listen --hotkey F8
```

### 完了条件

YMM4再生中に、

```text
F8
喋る
F8
```

で音声・開始フレーム・終了フレームが保存される。

まだ文字起こし不要。

---

## PR 4: Capture Hostの耐障害性

追加：

- partial WAV recovery
- capture journal
- cancellation
- max recording duration
- max artifact size
- device disconnect
- duplicate hotkey suppression
- process restart recovery
- source drift判定

### 完了条件

録音中にCapture Hostを落としても、

- YMM4プロジェクトは無傷
- 不完全WAVを正式なCaptureとして扱わない
- 既存Captureを壊さない

こと。

---

## PR 5: YMM4 Tool Panel統合

変更：

```text
TakeGraphBridgeView.xaml
TakeGraphBridgeViewModel.cs
Annotation/CaptureHostClient.cs
```

実装：

- Capture Host status
- microphone selection
- start/stop
- latest annotations
- recording indicator
- jump
- dismiss
- Capture Host不在時の案内

### 完了条件

CLIを触らず、YMM4 panelだけで録音できる。

---

## PR 6: Transcription provider

追加：

```text
crates/takegraph-node/src/transcription.rs
crates/takegraph-node/src/whisper_cpp.rs
crates/takegraph-service/src/transcription_jobs.rs
```

実装：

- provider trait
- whisper adapter
- job persistence
- retry
- transcript revisions
- human correction
- provider/model digest

### 完了条件

ASRを止めてもCaptureが失われず、再起動後に再実行できる。

---

## PR 7: Studio Viewのreview UI

追加：

```text
apps/studio-view/src/features/annotations/
```

変更：

```text
apps/studio-view/src/host-bridge.ts
apps/mcp-server/src/...
```

実装：

- list
- audio playback
- transcript edit
- jump
- dismiss
- stale表示
- manual tag

### 完了条件

音声メモを一周見直して、不要なものを捨て、文字を直せる。

---

## PR 8: AI Interpretation

追加：

- interpretation schema
- model adapter
- structured output validation
- temporal phrase parser
- manual correction
- interpretation revisions

対象表現：

```text
今の
さっきの
三秒前から
ここから
この後
ここ全部
ここは説明
残す
切る
確認
```

### 完了条件

雑な発話から、

```text
intent
relative range
topic
confidence
```

が候補として出る。

この段階ではまだYMM4を変更しない。

---

## PR 9: Narration promotion

変更：

```text
timeline_edit.rs
managed_cue.rs
facade schemas
Studio UI
```

追加：

- `sourceEvidence`
- ScriptCandidate
- narration placement
- timeline edit staging
- stale check
- promotion event
- Quint protocol

### 完了条件

Annotationから作った台本を、

```text
timeline_edit
```

としてstageし、既存のdigest-bound executeでYMM4へ入れられる。

---

# テスト項目

## Core

- frame < 0を拒否
- fps = 0を拒否
- empty hashを拒否
- start/end target不一致を`SourceChanged`
- audio metadata不整合を拒否
- Captureの上書きを拒否
- Transcriptが別audio hashを参照したら拒否
- Interpretationが古いtranscript digestならstale

## Capture Host

- start時にYMM4不在なら録音開始しない
- 録音中にYMM4が閉じても音声は保存
- マイク切断
- 二重start
- stopなしの終了
- 空音声
- 最大時間超過
- WAV flush失敗
- CAS collision
- 同一hotkeyのautorepeat

## YMM4 integration

- 録音開始で再生停止しない
- タイムラインfingerprintを変更しない
- dirty stateを変更しない
- selectionを変更しない
- annotation jump以外ではseekしない
- 別project/sceneへ誤ジャンプしない

## Service

- Capture追加でcanonical headが変わらない
- process restart後もannotationを読める
- 同一Capture再送はidempotent
- 異なる内容の同一IDはconflict
- stale sourceからのpromotionを拒否
- verified execute後だけPromotedになる

## MCP

- model-facing結果にローカルpathを出さない
- capture-host tokenを出さない
- ASR model pathを出さない
- `annotations` inspect viewがstable IDを返す
- timeline editへsource evidenceが封印される
- agent skillとinspect view一覧を同期する

---

# 第一号でやらないこと

これは明確に切った方がいい。

- 自動カット
- ripple delete
- 常時録音
- VLMによる全動画解析
- 自動で完成台本を確定
- Annotationから直接YMM4を変更
- cloud ASRへの自動アップロード
- unsaved/untitled project対応
- 押しっぱなしglobal PTT
- 自動B-roll
- 音声尺に合わせた映像の自動伸縮
- マルチNLE対応

全部あとでいい。

最初の価値は、

> **動画を止めずに喋ると、正しい時刻へ結び付いた音声メモが残る**

これだけで成立する。

---

# 最初の完成ライン

第一マイルストーンは次の10項目よ。

1. YMM4の動画を再生し続けられる  
2. ホットキーで録音開始・停止できる  
3. 開始・終了frameが保存される  
4. raw WAVがimmutableに保存される  
5. Capture Host再起動後も残る  
6. YMM4 panelに一覧表示される  
7. クリックすると元のframeへ戻れる  
8. ASRが失敗しても音声は失われない  
9. annotation追加ではcanonical revisionが進まない  
10. YMM4のタイムライン・dirty stateを変更しない  

ここまでできた時点で、すでに使える。

その後に、

```text
文字起こし
→ 意図解釈
→ 台本候補
→ timeline_edit
```

を一本ずつ足す。

一番最初に切るべき変更は **`docs/voice-annotation.md` と `annotation.rs`** ね。  
ただし最初の実用的な縦切りは、**既存の `current_scene_composition()` を使った Capture Host + トグル録音**。ここを先に通せば、アイデアが本当に気持ちいいかを、AI部分を一行も作らずに検証できるわ。

---
Powered by [AI Exporter](https://saveai.net)