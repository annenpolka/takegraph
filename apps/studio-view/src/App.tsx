import { useEffect, useMemo, useState } from "react";
import {
  createStudioHostBridge,
  type ProjectSummary,
} from "./host-bridge";

const utterances = [
  { speaker: "魔理沙", caption: "ここから第二形態だぜ", start: "00:12.450" },
  { speaker: "霊夢", caption: "聞いてないよ", start: "00:14.820" },
  { speaker: "魔理沙", caption: "一度、距離を取ろう", start: "00:17.260" },
];

const takes = [
  { id: "A", duration: "2.03s", speed: "1.00", active: false },
  { id: "B", duration: "1.84s", speed: "1.12", active: true },
  { id: "C", duration: "1.81s", speed: "1.14", active: false },
];

function formatDuration(durationMs: number): string {
  return `${(durationMs / 1000).toFixed(1)}s`;
}

export function App() {
  const bridge = useMemo(() => createStudioHostBridge(), []);
  const [summary, setSummary] = useState<ProjectSummary>();
  const [error, setError] = useState<string>();

  useEffect(() => {
    let active = true;
    bridge
      .loadProjectSummary()
      .then((next) => active && setSummary(next))
      .catch((reason: unknown) => {
        if (active) {
          setError(reason instanceof Error ? reason.message : String(reason));
        }
      });
    return () => {
      active = false;
    };
  }, [bridge]);

  return (
    <main className="studio-shell">
      <header className="topbar">
        <div className="brand-lockup">
          <span className="brand-mark" aria-hidden="true">TG</span>
          <div>
            <strong>TakeGraph</strong>
            <span>Every take, traceable.</span>
          </div>
        </div>
        <div className="project-meta">
          <span>{summary?.projectName ?? "Loading project…"}</span>
          <span className="revision">r{summary?.revision ?? "—"}</span>
          <span className={`engine ${summary?.voiceEngine ?? "unavailable"}`}>
            VOICEVOX {summary?.voiceEngine === "connected" ? "online" : "offline"}
          </span>
          <span className="mode">{bridge.mode}</span>
        </div>
      </header>

      {error && <div className="error-banner">Project service: {error}</div>}

      <section className="workspace">
        <aside className="script-panel panel">
          <div className="panel-heading">
            <div>
              <span className="eyebrow">SCRIPT GRAPH</span>
              <h1>台本</h1>
            </div>
            <button type="button" className="icon-button" aria-label="Add utterance">＋</button>
          </div>
          <div className="utterance-list">
            {utterances.map((utterance, index) => (
              <button
                className={`utterance ${index === 0 ? "selected" : ""}`}
                key={utterance.start}
                type="button"
              >
                <span className="utterance-index">{String(index + 1).padStart(2, "0")}</span>
                <span className="utterance-body">
                  <span className="utterance-row">
                    <strong>{utterance.speaker}</strong>
                    <time>{utterance.start}</time>
                  </span>
                  <span>{utterance.caption}</span>
                </span>
              </button>
            ))}
          </div>
        </aside>

        <section className="preview-column">
          <div className="preview panel">
            <div className="preview-toolbar">
              <span className="eyebrow">REFERENCE PREVIEW</span>
              <div className="segmented"><button className="active">Current</button><button>Ghost</button></div>
            </div>
            <div className="stage">
              <div className="stage-grid" />
              <div className="scene-label">BOSS / PHASE 02</div>
              <div className="caption-preview">ここから第二形態だぜ</div>
            </div>
            <div className="transport">
              <button type="button" aria-label="Previous frame">◀│</button>
              <button type="button" className="play" aria-label="Play">▶</button>
              <button type="button" aria-label="Next frame">│▶</button>
              <span className="timecode">00:12:45 / {summary ? formatDuration(summary.durationMs) : "—"}</span>
            </div>
          </div>

          <div className="timeline panel">
            <div className="timeline-heading">
              <div><span className="eyebrow">PATCH IMPACT</span><strong>Ghost Timeline</strong></div>
              <span className="impact">−190ms · 5 items ripple</span>
            </div>
            <div className="ruler"><span>12s</span><span>14s</span><span>16s</span><span>18s</span><span>20s</span></div>
            <div className="tracks">
              <span className="playhead" />
              <div className="track"><label>VIDEO</label><div className="clip video-clip">gameplay-main</div></div>
              <div className="track"><label>VOICE</label><div className="clip voice-clip current">Take A</div><div className="clip voice-clip ghost">Take B</div></div>
              <div className="track"><label>CAPTION</label><div className="clip caption-clip">第二形態だぜ</div></div>
            </div>
          </div>
        </section>

        <aside className="voice-panel panel">
          <div className="panel-heading">
            <div><span className="eyebrow">VOICE AUDITION</span><h2>音声テイク</h2></div>
            <span className="speaker-pill">魔理沙</span>
          </div>
          <label className="field-label" htmlFor="spoken-text">読み上げ文</label>
          <textarea id="spoken-text" defaultValue="ここからだいにけいたいだぜ" />
          <div className="controls-grid">
            <label>話速 <input type="range" min="0.5" max="2" step="0.01" defaultValue="1.12" /><output>1.12</output></label>
            <label>抑揚 <input type="range" min="0" max="2" step="0.01" defaultValue="0.95" /><output>0.95</output></label>
          </div>
          <div className="take-list">
            {takes.map((take) => (
              <button className={`take ${take.active ? "active" : ""}`} key={take.id} type="button">
                <span className="take-play">▶</span>
                <strong>Take {take.id}</strong>
                <span>{take.duration}</span>
                <span>speed {take.speed}</span>
                {take.active && <em>selected</em>}
              </button>
            ))}
          </div>
          <div className="impact-card">
            <span>採用時の影響</span>
            <strong>後続5項目を190ms前へ移動</strong>
            <small>字幕改行ロックは維持されます</small>
          </div>
          <div className="actions">
            <button type="button" className="secondary">候補を生成</button>
            <button type="button" className="primary">Take Bを採用</button>
          </div>
        </aside>
      </section>
    </main>
  );
}

