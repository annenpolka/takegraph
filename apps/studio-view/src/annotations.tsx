import type { AnnotationRow } from "./host-bridge";

function intentKinds(row: AnnotationRow): string {
  return row.intents.map((intent) => intent.kind).join(", ") || "none";
}

function temporalText(row: AnnotationRow): string {
  const temporal = row.temporal;
  if (!temporal) return "none";
  return `${temporal.relation}@${temporal.referenceFrame}:${temporal.startOffsetFrames}`;
}

function deriveLabel(row: AnnotationRow): string {
  if (row.derivePhase === "queued" || row.derivePhase === "running") return "起こし中";
  if (row.derivePhase === "failed") return "起こし失敗";
  return row.transcriptSummary ?? "(no transcript)";
}

export function AnnotationsPanel(props: {
  active: boolean;
  annotations: AnnotationRow[];
  selectedId?: string;
  draft: string;
  characterName: string;
  layer: number;
  busy: boolean;
  onSelect(id: string): void;
  onDraft(value: string): void;
  onCharacterName(value: string): void;
  onLayer(value: number): void;
  onRefresh(): void;
  onCorrect(): void;
  onDismiss(): void;
  onInterpret(): void;
  onPromote(): void;
  onPin(): void;
  onUnpin(): void;
}) {
  const selected = props.annotations.find((row) => row.annotationId === props.selectedId);
  return (
    <aside
      id="region-notes"
      className={`notes-panel panel studio-region ${props.active ? "is-active" : ""}`}
      role="tabpanel"
      aria-labelledby="tab-notes"
    >
      <div className="panel-heading">
        <div>
          <span className="eyebrow">VOICE NOTES</span>
          <h2>アノテーション</h2>
        </div>
        <button type="button" className="icon-button" disabled={props.busy} onClick={props.onRefresh} title="再読込">
          ↻
        </button>
      </div>
      <div className="notes-body">
        <div className="utterance-list">
          {props.annotations.length === 0 && (
            <p className="empty-state">キャプチャはまだありません。</p>
          )}
          {props.annotations.map((row) => (
            <button
              className={`utterance ${row.annotationId === props.selectedId ? "selected" : ""}`}
              key={row.annotationId}
              type="button"
              onClick={() => props.onSelect(row.annotationId)}
            >
              <span className="utterance-index">
                {String(row.startFrame).padStart(4, "0")}
              </span>
              <span className="utterance-body">
                <span className="utterance-row">
                  <strong>{intentKinds(row)}</strong>
                  <time>
                    {row.stale ? "stale" : row.lifecycle}
                    {row.promotionStatus ? ` / ${row.promotionStatus}` : ""}
                    {row.derivePhase ? ` / ${row.derivePhase}` : ""}
                  </time>
                </span>
                <span>{deriveLabel(row)}</span>
              </span>
            </button>
          ))}
        </div>
        <div className="notes-detail">
          {selected ? (
            <>
              <p className="notes-meta">
                frames {selected.startFrame}-{selected.endFrame} · {selected.stability} · temporal{" "}
                {temporalText(selected)}
                {selected.promotionStatus
                  ? ` · promotion ${selected.promotionStatus}`
                  : ""}
              </p>
              <label className="field-label" htmlFor="annotation-transcript">
                文字起こし
              </label>
              <textarea
                id="annotation-transcript"
                value={props.draft}
                onChange={(event) => props.onDraft(event.target.value)}
              />
              <div className="notes-actions">
                <button type="button" className="secondary" disabled={props.busy} onClick={props.onCorrect}>
                  文字を直す
                </button>
                <button type="button" className="secondary" disabled={props.busy} onClick={props.onInterpret}>
                  解釈する
                </button>
                <button type="button" className="secondary" disabled={props.busy} onClick={props.onDismiss}>
                  捨てる
                </button>
              </div>
              <label className="field-label" htmlFor="annotation-character">
                昇格キャラ
              </label>
              <input
                id="annotation-character"
                value={props.characterName}
                onChange={(event) => props.onCharacterName(event.target.value)}
              />
              <label className="field-label" htmlFor="annotation-layer">
                レイヤ
              </label>
              <input
                id="annotation-layer"
                type="number"
                min={0}
                value={props.layer}
                onChange={(event) => props.onLayer(Number(event.target.value))}
              />
              <button
                type="button"
                className="primary"
                disabled={
                  props.busy ||
                  !props.characterName.trim() ||
                  selected.promotionStatus === "staged" ||
                  selected.promotionStatus === "committed"
                }
                onClick={props.onPromote}
              >
                {selected.promotionStatus === "staged"
                  ? "timeline_edit をステージ済み"
                  : selected.promotionStatus === "committed"
                    ? "昇格済み"
                    : "ナレーションを timeline_edit へ"}
              </button>
              <button
                type="button"
                className="secondary"
                disabled={
                  props.busy ||
                  !selected.interpretationDigest ||
                  selected.lifecycle !== "active" ||
                  selected.stability === "source_changed" ||
                  selected.promotionStatus === "staged" ||
                  Boolean(selected.pinEntityId)
                }
                onClick={props.onPin}
              >
                {selected.pinEntityId ? "ピン済み" : "メモをピン"}
              </button>
              <button
                type="button"
                className="secondary"
                disabled={
                  props.busy ||
                  !selected.pinEntityId ||
                  selected.promotionStatus === "staged"
                }
                onClick={props.onUnpin}
              >
                ピンを外す
              </button>
              <small className="notes-hint">
                {selected.promotionPlanDigest
                  ? `planDigest ${selected.promotionPlanDigest}. 実行は既存の timeline_edit。`
                  : selected.pinEntityId
                    ? `pin ${selected.pinEntityId}. 外すときも timeline_edit。`
                    : "CutCandidate は証拠のまま残します。音声パスは出しません。"}
              </small>
            </>
          ) : (
            <p className="empty-state">メモを選ぶと文字と intent を見直せます。</p>
          )}
        </div>
      </div>
    </aside>
  );
}
