import { useEffect, useMemo, useState } from "react";
import { AnnotationsPanel } from "./annotations";
import {
  createStudioHostBridge,
  type AnnotationRow,
  type ProjectState,
} from "./host-bridge";

type StudioRegion = "script" | "preview" | "voice" | "notes";

function formatDuration(durationMs: number): string {
  return `${(durationMs / 1_000).toFixed(2)}s`;
}

function formatTime(startMs: number): string {
  const seconds = Math.floor(startMs / 1_000);
  return `00:${String(seconds).padStart(2, "0")}.${String(startMs % 1_000).padStart(3, "0")}`;
}

export function App() {
  const bridge = useMemo(() => createStudioHostBridge(), []);
  const [state, setState] = useState<ProjectState>();
  const [selectedUtteranceId, setSelectedUtteranceId] = useState<string>();
  const [selectedTakeId, setSelectedTakeId] = useState<string>();
  const [speed, setSpeed] = useState(1.12);
  const [intonation, setIntonation] = useState(0.95);
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);
  const [activeRegion, setActiveRegion] = useState<StudioRegion>("preview");
  const [annotations, setAnnotations] = useState<AnnotationRow[]>([]);
  const [selectedAnnotationId, setSelectedAnnotationId] = useState<string>();
  const [annotationDraft, setAnnotationDraft] = useState("");
  const [promoteCharacter, setPromoteCharacter] = useState("ゆっくり霊夢");
  const [promoteLayer, setPromoteLayer] = useState(2);

  useEffect(() => {
    let active = true;
    const unsubscribe = bridge.subscribe((next) => active && setState(next));
    bridge
      .loadProjectState()
      .then((next) => active && setState(next))
      .catch((reason: unknown) => {
        if (active) {
          setError(reason instanceof Error ? reason.message : String(reason));
        }
      });
    return () => {
      active = false;
      unsubscribe();
    };
  }, [bridge]);

  useEffect(() => {
    if (!state) return;
    setSelectedUtteranceId((current) => current ?? state.utterances[0]?.id);
  }, [state]);

  const utterance = state?.utterances.find(
    (candidate) => candidate.id === selectedUtteranceId,
  );
  const takes = state?.takes.filter(
    (candidate) => candidate.utteranceId === selectedUtteranceId,
  );

  useEffect(() => {
    if (!state || !selectedUtteranceId) return;
    const available = state.takes.filter(
      (take) => take.utteranceId === selectedUtteranceId,
    );
    setSelectedTakeId((current) => {
      if (available.some((take) => take.id === current)) return current;
      return available.find((take) => take.id === state.activeTakeId)?.id ?? available[0]?.id;
    });
  }, [selectedUtteranceId, state]);

  const selectedTake = takes?.find((take) => take.id === selectedTakeId);
  const stagedPatch = state?.stagedPatch;

  async function run(action: () => Promise<ProjectState>): Promise<ProjectState | undefined> {
    setBusy(true);
    setError(undefined);
    try {
      const next = await action();
      setState(next);
      return next;
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      return undefined;
    } finally {
      setBusy(false);
    }
  }

  async function generateVariant() {
    if (!utterance) return;
    const before = new Set(state?.takes.map((take) => take.id));
    const next = await run(() =>
      bridge.generateVariant({ utteranceId: utterance.id, speed, intonation }),
    );
    const created = next?.takes.find((take) => !before.has(take.id));
    if (created) setSelectedTakeId(created.id);
  }

  async function selectUtterance(id: string) {
    setSelectedUtteranceId(id);
    const next = state?.utterances.find((candidate) => candidate.id === id);
    if (next) await bridge.focusUtterance(next).catch(() => undefined);
  }

  async function refreshAnnotations() {
    setBusy(true);
    setError(undefined);
    try {
      const rows = await bridge.listAnnotations();
      setAnnotations(rows);
      setSelectedAnnotationId((current) => {
        if (current && rows.some((row) => row.annotationId === current)) return current;
        return rows[0]?.annotationId;
      });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    const selected = annotations.find((row) => row.annotationId === selectedAnnotationId);
    setAnnotationDraft(selected?.transcriptSummary ?? "");
  }, [annotations, selectedAnnotationId]);

  async function runAnnotation(
    action: () => Promise<AnnotationRow[]>,
  ): Promise<void> {
    setBusy(true);
    setError(undefined);
    try {
      setAnnotations(await action());
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  }

  async function stageSelectedTake() {
    if (!selectedTake) return;
    const next = await run(() => bridge.stageTake(selectedTake.id));
    if (next?.stagedPatch) setActiveRegion("preview");
  }

  const impactText = stagedPatch
    ? `${stagedPatch.durationDeltaMs > 0 ? "+" : ""}${stagedPatch.durationDeltaMs}ms · ${stagedPatch.movedItemCount} items ripple`
    : "変更はまだステージされていません";

  return (
    <main className="studio-shell" aria-busy={busy}>
      <header className="topbar">
        <div className="brand-lockup">
          <span className="brand-mark" aria-hidden="true">TG</span>
          <div><strong>TakeGraph</strong><span>Every take, traceable.</span></div>
        </div>
        <div className="project-meta">
          <span className="project-name">{state?.projectName ?? "Loading project…"}</span>
          <span className="revision">r{state?.revision ?? "—"}</span>
          <span className={`engine ${state?.voiceEngine ?? "unavailable"}`}>
            VOICEVOX {state?.voiceEngine === "connected" ? "online" : "offline"}
          </span>
          <span className="mode">{bridge.mode}</span>
          {bridge.mode === "mcp" && (
            <button className="fullscreen" type="button" onClick={() => void bridge.requestFullscreen()}>
              拡大 ↗
            </button>
          )}
        </div>
      </header>

      {error && <div className="error-banner" role="alert">Project service: {error}</div>}
      {busy && <div className="status-banner" role="status">TakeGraph tool is running…</div>}

      <nav className="inline-tabs" aria-label="Editor sections" role="tablist">
        <button
          id="tab-script"
          type="button"
          role="tab"
          aria-controls="region-script"
          aria-selected={activeRegion === "script"}
          className={activeRegion === "script" ? "active" : ""}
          onClick={() => setActiveRegion("script")}
        >
          <span>台本</span>
          <em>{state?.utterances.length ?? "—"}</em>
        </button>
        <button
          id="tab-preview"
          type="button"
          role="tab"
          aria-controls="region-preview"
          aria-selected={activeRegion === "preview"}
          className={activeRegion === "preview" ? "active" : ""}
          onClick={() => setActiveRegion("preview")}
        >
          <span>プレビュー</span>
          {stagedPatch && <i aria-label="patch staged" />}
        </button>
        <button
          id="tab-voice"
          type="button"
          role="tab"
          aria-controls="region-voice"
          aria-selected={activeRegion === "voice"}
          className={activeRegion === "voice" ? "active" : ""}
          onClick={() => setActiveRegion("voice")}
        >
          <span>音声</span>
          <em>{takes?.length ?? "—"}</em>
        </button>
        <button
          id="tab-notes"
          type="button"
          role="tab"
          aria-controls="region-notes"
          aria-selected={activeRegion === "notes"}
          className={activeRegion === "notes" ? "active" : ""}
          onClick={() => {
            setActiveRegion("notes");
            void refreshAnnotations();
          }}
        >
          <span>メモ</span>
          <em>{annotations.length}</em>
        </button>
      </nav>

      <section className="workspace">
        <aside
          id="region-script"
          className={`script-panel panel studio-region ${activeRegion === "script" ? "is-active" : ""}`}
          role="tabpanel"
          aria-labelledby="tab-script"
        >
          <div className="panel-heading">
            <div><span className="eyebrow">SCRIPT GRAPH</span><h1>台本</h1></div>
            <button type="button" className="icon-button" disabled title="台本編集は次のマイルストーンです">＋</button>
          </div>
          <div className="utterance-list">
            {state?.utterances.map((item, index) => (
              <button
                className={`utterance ${item.id === selectedUtteranceId ? "selected" : ""}`}
                key={item.id}
                type="button"
                onClick={() => void selectUtterance(item.id)}
              >
                <span className="utterance-index">{String(index + 1).padStart(2, "0")}</span>
                <span className="utterance-body">
                  <span className="utterance-row"><strong>{item.speaker}</strong><time>{formatTime(item.startMs)}</time></span>
                  <span>{item.caption}</span>
                </span>
              </button>
            ))}
          </div>
        </aside>

        <section
          id="region-preview"
          className={`preview-column studio-region ${activeRegion === "preview" ? "is-active" : ""}`}
          role="tabpanel"
          aria-labelledby="tab-preview"
        >
          <div className="preview panel">
            <div className="preview-toolbar">
              <span className="eyebrow">REFERENCE PREVIEW</span>
              <div className="segmented"><span className="active">Current</span><span>Ghost</span></div>
            </div>
            <div className="stage">
              <div className="stage-grid" />
              <div className="scene-label">BOSS / PHASE 02</div>
              <div className="caption-preview">{utterance?.caption ?? "Project loading…"}</div>
            </div>
            <div className="transport">
              <button type="button" disabled aria-label="Previous frame">◀│</button>
              <button type="button" disabled className="play" aria-label="Play">▶</button>
              <button type="button" disabled aria-label="Next frame">│▶</button>
              <span className="timecode">{utterance ? formatTime(utterance.startMs) : "—"} / {state ? formatDuration(state.durationMs) : "—"}</span>
            </div>
          </div>

          <div className="timeline panel">
            <div className="timeline-heading">
              <div><span className="eyebrow">PATCH IMPACT</span><strong>Ghost Timeline</strong></div>
              <span className="impact">{impactText}</span>
            </div>
            <div className="ruler"><span>12s</span><span>14s</span><span>16s</span><span>18s</span><span>20s</span></div>
            <div className="tracks">
              <span className="playhead" />
              <div className="track"><label>VIDEO</label><div className="clip video-clip">gameplay-main</div></div>
              <div className="track"><label>VOICE</label><div className="clip voice-clip current">Take {state?.takes.find((take) => take.id === state.activeTakeId)?.label ?? "—"}</div>{stagedPatch && <div className="clip voice-clip ghost">Take {state?.takes.find((take) => take.id === stagedPatch.takeId)?.label}</div>}</div>
              <div className="track"><label>CAPTION</label><div className="clip caption-clip">{utterance?.caption ?? "—"}</div></div>
            </div>
          </div>
        </section>

        <aside
          id="region-voice"
          className={`voice-panel panel studio-region ${activeRegion === "voice" ? "is-active" : ""}`}
          role="tabpanel"
          aria-labelledby="tab-voice"
        >
          <div className="panel-heading">
            <div><span className="eyebrow">VOICE AUDITION</span><h2>音声テイク</h2></div>
            <span className="speaker-pill">{utterance?.speaker ?? "—"}</span>
          </div>
          <label className="field-label" htmlFor="spoken-text">読み上げ文</label>
          <textarea id="spoken-text" readOnly value={utterance?.spokenText ?? ""} />
          <div className="controls-grid">
            <label>話速 <input aria-label="話速" type="range" min="0.5" max="2" step="0.01" value={speed} onChange={(event) => setSpeed(Number(event.target.value))} /><output>{speed.toFixed(2)}</output></label>
            <label>抑揚 <input aria-label="抑揚" type="range" min="0" max="2" step="0.01" value={intonation} onChange={(event) => setIntonation(Number(event.target.value))} /><output>{intonation.toFixed(2)}</output></label>
          </div>
          <div className="take-list">
            {takes?.map((take) => (
              <button className={`take ${take.id === selectedTakeId ? "active" : ""}`} key={take.id} type="button" onClick={() => setSelectedTakeId(take.id)}>
                <span className="take-play">◆</span>
                <strong>Take {take.label}</strong>
                <span>{formatDuration(take.durationMs)}</span>
                <span>speed {take.speed.toFixed(2)}</span>
                <em>{take.status === "active" ? "project active" : take.readiness}</em>
              </button>
            ))}
            {takes?.length === 0 && <p className="empty-state">この発話にはテイクがありません。</p>}
          </div>
          <div className="impact-card">
            <span>{stagedPatch ? "採用パッチをプレビュー中" : "採用時の影響"}</span>
            <strong>{stagedPatch ? `${stagedPatch.movedItemCount}項目を${Math.abs(stagedPatch.durationDeltaMs)}ms移動` : "テイクを選び、影響を計算します"}</strong>
            <small>{stagedPatch ? `digest ${stagedPatch.digest.slice(0, 12)}… / base r${stagedPatch.baseRevision}` : "commit 前に exact digest を承認します"}</small>
          </div>
          <div className="actions">
            <button type="button" className="secondary" disabled={busy || !utterance} onClick={() => void generateVariant()}>候補を作成</button>
            <button type="button" className="primary" disabled={busy || Boolean(stagedPatch) || !selectedTake || selectedTake.readiness !== "ready" || selectedTake.status === "active"} onClick={() => void stageSelectedTake()}>
              {stagedPatch ? "Preview中" : selectedTake?.readiness === "query-ready" ? "音声artifact待ち" : `Take ${selectedTake?.label ?? "—"}をプレビュー`}
            </button>
          </div>
          {stagedPatch && (
            <div className="commit-bar">
              <span>この exact patch を承認</span>
              <button type="button" disabled={busy} onClick={() => void run(() => bridge.commitPatch(stagedPatch.id, stagedPatch.digest))}>Commit patch</button>
            </div>
          )}
        </aside>
        <AnnotationsPanel
          active={activeRegion === "notes"}
          annotations={annotations}
          selectedId={selectedAnnotationId}
          draft={annotationDraft}
          characterName={promoteCharacter}
          layer={promoteLayer}
          busy={busy}
          onSelect={setSelectedAnnotationId}
          onDraft={setAnnotationDraft}
          onCharacterName={setPromoteCharacter}
          onLayer={setPromoteLayer}
          onRefresh={() => void refreshAnnotations()}
          onCorrect={() => {
            if (!selectedAnnotationId) return;
            void runAnnotation(() =>
              bridge.correctAnnotation(selectedAnnotationId, annotationDraft),
            );
          }}
          onDismiss={() => {
            if (!selectedAnnotationId) return;
            void runAnnotation(() => bridge.dismissAnnotation(selectedAnnotationId));
          }}
          onInterpret={() => {
            if (!selectedAnnotationId) return;
            void runAnnotation(() => bridge.interpretAnnotation(selectedAnnotationId));
          }}
          onPromote={() => {
            if (!selectedAnnotationId) return;
            void run(async () => {
              await bridge.promoteAnnotation({
                annotationId: selectedAnnotationId,
                characterName: promoteCharacter,
                layer: promoteLayer,
              });
              const rows = await bridge.listAnnotations();
              setAnnotations(rows);
              return state ?? (await bridge.loadProjectState());
            });
          }}
          onPin={() => {
            if (!selectedAnnotationId) return;
            void run(async () => {
              await bridge.pinAnnotation(selectedAnnotationId);
              const rows = await bridge.listAnnotations();
              setAnnotations(rows);
              return state ?? (await bridge.loadProjectState());
            });
          }}
          onUnpin={() => {
            if (!selectedAnnotationId) return;
            void run(async () => {
              await bridge.unpinAnnotation(selectedAnnotationId);
              const rows = await bridge.listAnnotations();
              setAnnotations(rows);
              return state ?? (await bridge.loadProjectState());
            });
          }}
        />
      </section>
    </main>
  );
}
