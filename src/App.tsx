import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

import { usePhotos } from "./usePhotos";
import type { Photo, View } from "./types";
import "./app.css";

/**
 * The filmstrip cell. Points at the 400px thumb, never the overlay PNG: Tauri's
 * asset protocol reads a whole file into memory per request, and the print output
 * is 10-25 MB and 16-bit when the source was HEIC.
 *
 * Selection and status are separate channels on purpose. Selection is the border;
 * status is a dot in the corner. An earlier version put both on the border, and a
 * selected photo that had a warning simply showed no selection.
 */
function Cell({ photo, active, onSelect }: { photo: Photo; active: boolean; onSelect: () => void }) {
  const status = photo.status === "failed" ? "failed" : photo.warnings.length > 0 ? "warned" : null;
  const tooltip = photo.error || photo.warnings[0] || photo.name;

  return (
    <button
      className={`cell ${active ? "is-selected" : ""}`}
      onClick={onSelect}
      title={tooltip}
      aria-pressed={active}
      data-source={photo.source}
    >
      <span className="cell__frame">
        {photo.previews ? (
          <img src={convertFileSrc(photo.previews.thumb)} alt="" draggable={false} />
        ) : (
          <span className={`cell__placeholder ${photo.status === "processing" ? "is-working" : ""}`} />
        )}
        {status && <span className={`cell__badge cell__badge--${status}`} aria-label={status} />}
      </span>
      <span className="cell__name">{photo.name}</span>
    </button>
  );
}

function folderName(path: string) {
  return path.replace(/\/+$/, "").split("/").pop() || path;
}

export default function App() {
  const { photos, settings, runtime, selected, setSelected, reprocess, changeWatchDir } = usePhotos();
  const [view, setView] = useState<View>("overlay");
  const [busy, setBusy] = useState<"share" | "print" | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const shareButton = useRef<HTMLButtonElement>(null);

  const ordered = useMemo(() => [...photos].sort((a, b) => a.added_at - b.added_at), [photos]);
  const current = ordered.find((p) => p.source === selected) ?? null;
  const ready = current?.status === "done" && !!current.previews;

  const step = useCallback(
    (by: number) => {
      if (ordered.length === 0) return;
      const at = ordered.findIndex((p) => p.source === selected);
      const next = Math.min(Math.max((at === -1 ? 0 : at) + by, 0), ordered.length - 1);
      setSelected(ordered[next].source);
    },
    [ordered, selected, setSelected],
  );

  // Keep the selected cell on screen, which the keyboard would otherwise walk
  // straight off the edge of.
  useEffect(() => {
    if (!selected) return;
    document
      .querySelector(`[data-source="${CSS.escape(selected)}"]`)
      ?.scrollIntoView({ inline: "nearest", block: "nearest", behavior: "smooth" });
  }, [selected]);

  const act = useCallback(
    async (kind: "share" | "print") => {
      if (!current || !ready || busy) return;
      setBusy(kind);
      setProblem(null);
      try {
        if (kind === "print") {
          await invoke("print_photo", { source: current.source });
        } else {
          // The sheet is anchored to the button, so it needs to know where that is.
          const box = shareButton.current?.getBoundingClientRect();
          const anchor = box
            ? { x: box.left, y: box.top, width: box.width, height: box.height }
            : { x: window.innerWidth / 2, y: window.innerHeight / 2, width: 1, height: 1 };
          await invoke("share_photo", { source: current.source, anchor });
        }
      } catch (error) {
        setProblem(String(error));
      } finally {
        setBusy(null);
      }
    },
    [current, ready, busy],
  );

  // Lightroom's before/after key is \, so it is the one an operator's hands will
  // already know. Arrows move along the strip; Cmd+P prints, as everywhere.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        step(event.key === "ArrowLeft" ? -1 : 1);
      } else if (event.key === "\\") {
        event.preventDefault();
        setView((v) => (v === "overlay" ? "source" : "overlay"));
      } else if (event.key === "p" && event.metaKey) {
        event.preventDefault();
        act("print");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [step, act]);

  const pickWatchDir = async () => {
    const chosen = await open({ directory: true, multiple: false, defaultPath: settings?.watch_dir });
    if (typeof chosen === "string") await changeWatchDir(chosen);
  };

  const preview = current?.previews?.[view];

  return (
    <div className="app">
      {/* data-tauri-drag-region is what makes a transparent titlebar's row
          draggable; without it the only grab handle is the sliver above it. */}
      <header className="bar" data-tauri-drag-region>
        <button
          className="bar__dir"
          onClick={pickWatchDir}
          title={settings ? `Watching ${settings.watch_dir} - click to change` : ""}
        >
          <span className="bar__dir-icon" aria-hidden="true" />
          {settings ? folderName(settings.watch_dir) : "..."}
        </button>
        <span className="bar__count">
          {ordered.length} photo{ordered.length === 1 ? "" : "s"}
        </span>
        <div className="toggle" role="group" aria-label="Which version to show">
          <button className={view === "overlay" ? "is-on" : ""} onClick={() => setView("overlay")} disabled={!ready} aria-pressed={view === "overlay"}>
            Overlayed
          </button>
          <button className={view === "source" ? "is-on" : ""} onClick={() => setView("source")} disabled={!ready} aria-pressed={view === "source"}>
            Original
          </button>
        </div>
      </header>

      <main className="stage">
        <div className="stage__canvas">
          {(() => {
            if (!runtime.ready) {
              return (
                <p className="notice">
                  {runtime.error ?? "Starting the image pipeline..."}
                  {runtime.error && <span className="notice__hint">Run `mise run runtime` and restart the app.</span>}
                </p>
              );
            }
            if (!current) {
              return <p className="notice">AirDrop a photo to {settings ? folderName(settings.watch_dir) : "the watched folder"} to get started.</p>;
            }
            if (preview) {
              return <img className="stage__image" src={convertFileSrc(preview)} alt={current.name} draggable={false} />;
            }
            if (current.status === "failed") {
              return (
                <p className="notice notice--failed">
                  <strong>{current.name}</strong>
                  {current.error}
                  <button className="button" onClick={() => reprocess(current.source)}>Try again</button>
                </p>
              );
            }
            return <p className="notice">Working on {current.name}...</p>;
          })()}
        </div>

        <footer className="actions">
          <div className="actions__info">
            {current?.warnings[0] && <span className="actions__warning" title={current.warnings.join("\n")}>{current.warnings[0]}</span>}
            {problem && <span className="actions__problem">{problem}</span>}
            {ready && !current.warnings.length && !problem && (
              <span className="actions__meta">
                {current.output_width} x {current.output_height} px, {current.depth}-bit
                {current.bytes ? `, ${(current.bytes / 1024 / 1024).toFixed(1)} MB` : ""}
              </span>
            )}
          </div>
          <div className="actions__buttons">
            {current?.status === "failed" && (
              <button className="button" onClick={() => reprocess(current.source)}>Try again</button>
            )}
            <button className="button" onClick={() => current && invoke("reveal_photo", { source: current.source }).catch((e) => setProblem(String(e)))} disabled={!ready} title="Show the PNG in Finder">
              Reveal
            </button>
            <button ref={shareButton} className="button" onClick={() => act("share")} disabled={!ready || busy !== null}>
              {busy === "share" ? "Preparing..." : "Share"}
            </button>
            <button className="button button--primary" onClick={() => act("print")} disabled={!ready || busy !== null} title="Print (Cmd+P)">
              {busy === "print" ? "Opening..." : "Print"}
            </button>
          </div>
        </footer>
      </main>

      <nav className="strip" aria-label="Photos">
        {ordered.map((photo) => (
          <Cell key={photo.source} photo={photo} active={photo.source === selected} onSelect={() => setSelected(photo.source)} />
        ))}
      </nav>
    </div>
  );
}
