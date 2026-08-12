import { useEffect, useMemo, useState } from "react";
import {
  createStudioHostBridge,
  type ProjectState,
} from "./host-bridge";

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
          <span>{state?.projectName ?? "Loading project…"}</span>
          <span className="revision">r{state?.revision ?? "—"}</span>
          <span className={`engine ${state?.voiceEngine ?? "unavailable"}`}>
            VOICEVOX {state?.voiceEngine === "connected" ? "online" : "offline"}
          </span>
          <span className="mode">{bridge.mode}</span>
          {bridge.mode === "mcp" && (
            <button className="fullscreen" type="button" onClick={() => void bridge.requestFullscreen()}>
              Fullscreen
            </button>
          )}
        </div>
      </header>

      {error && <div className="error-banner" role="alert">Project service: {error}</div>}
      {busy && <div className="status-banner" role="status">TakeGraph tool is running…</div>}

      <section className="workspace">
        <aside className="script-panel panel">
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

        <section className="preview-column">
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

        <aside className="voice-panel panel">
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
            <button type="button" className="primary" disabled={busy || !selectedTake || selectedTake.readiness !== "ready" || selectedTake.status === "active"} onClick={() => selectedTake && void run(() => bridge.stageTake(selectedTake.id))}>
              {selectedTake?.readiness === "query-ready" ? "音声artifact待ち" : `Take ${selectedTake?.label ?? "—"}をプレビュー`}
            </button>
          </div>
          {stagedPatch && (
            <div className="commit-bar">
              <span>この exact patch を承認</span>
              <button type="button" disabled={busy} onClick={() => void run(() => bridge.commitPatch(stagedPatch.id, stagedPatch.digest))}>Commit patch</button>
            </div>
          )}
        </aside>
      </section>
    </main>
  );
}
