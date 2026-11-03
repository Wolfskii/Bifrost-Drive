import { useCallback, useEffect, useState } from "react";
import {
    answerTransferConflict,
    cancelTransfer,
    dismissTransfer,
    listTransfers,
    pauseTransfer,
    resumeTransfer,
    subscribeTransfers,
    type ConflictResolution,
    type TransferView,
} from "./api";
import {
    cardTitle,
    describeFailure,
    directionLabel,
    formatBytes,
    formatCount,
    formatRate,
    modeLabel,
    isFinished,
    percentDone,
    segmentFills,
    statusLine,
} from "./transferFormat";
import "./TransferWindow.css";

const MAX_LISTED_FAILURES = 50;

function badge(view: TransferView): {
    text: string;
    tone: string;
    note: string;
} {
    const { snapshot } = view;
    switch (snapshot.state) {
        case "paused":
            return { text: "❚❚ Paused", tone: "quiet", note: "" };
        case "waiting_for_answer":
            return { text: "? Needs a choice", tone: "quiet", note: "" };
        case "completed":
            return { text: "✓ Done", tone: "quiet", note: "" };
        case "completed_with_errors":
            return {
                text: `! ${formatCount(snapshot.failed_files)} failed`,
                tone: "danger",
                note: "see the details below",
            };
        case "cancelled":
            return { text: "Cancelled", tone: "quiet", note: "" };
        case "failed":
            return { text: "! Failed", tone: "danger", note: "" };
        case "planning":
            return { text: "…", tone: "quiet", note: "counting files" };
        default:
            return {
                text: `↗ ${formatRate(snapshot)}`,
                tone: "",
                note: "current speed",
            };
    }
}

function ConflictPrompt({
    view,
    onAnswer,
}: {
    view: TransferView;
    onAnswer: (resolution: ConflictResolution, all: boolean) => void;
}) {
    const [all, setAll] = useState(false);
    const conflict = view.snapshot.conflict;
    if (!conflict) return null;
    return (
        <div className="transfer-conflict" role="group" aria-label="Name clash">
            <p>
                “{conflict.name}” already exists in the destination.
                <span>
                    New file {formatBytes(conflict.source_bytes)} · existing
                    file {formatBytes(conflict.destination_bytes)}
                </span>
            </p>
            <label className="transfer-check">
                <input
                    type="checkbox"
                    checked={all}
                    onChange={(event) => setAll(event.target.checked)}
                />
                Do this for every name clash
            </label>
            <div className="transfer-actions">
                <button type="button" onClick={() => onAnswer("replace", all)}>
                    Replace
                </button>
                <button
                    type="button"
                    onClick={() => onAnswer("keep_both", all)}
                >
                    Keep both
                </button>
                <button type="button" onClick={() => onAnswer("skip", all)}>
                    Skip
                </button>
            </div>
        </div>
    );
}

export function TransferCard({
    view,
    onError,
    onDismissed,
}: {
    view: TransferView;
    onError: (message: string) => void;
    onDismissed: (id: number) => void;
}) {
    const [menuOpen, setMenuOpen] = useState(false);
    const { snapshot } = view;
    const finished = isFinished(snapshot);
    const percent = percentDone(snapshot);
    const state = badge(view);
    const title = cardTitle(view);

    const run = (action: Promise<void>) => {
        setMenuOpen(false);
        action.catch((cause: unknown) =>
            onError(
                cause instanceof Error
                    ? cause.message
                    : "That didn't work. Try again.",
            ),
        );
    };

    return (
        <article
            className={`transfer-card state-${snapshot.state}${
                snapshot.conflict ? " has-conflict" : ""
            }`}
        >
            <header className="transfer-head">
                <ul className="transfer-chips" aria-label="Transfer details">
                    {view.connection && <li>{view.connection}</li>}
                    <li>{modeLabel(snapshot.mode)}</li>
                    {snapshot.mode !== "delete" && (
                        <li>{directionLabel(view.direction)}</li>
                    )}
                </ul>
                <div className="transfer-menu">
                    <button
                        type="button"
                        className="transfer-more"
                        aria-label="Transfer options"
                        aria-haspopup="menu"
                        aria-expanded={menuOpen}
                        onClick={() => setMenuOpen((open) => !open)}
                    >
                        …
                    </button>
                    {menuOpen && (
                        <div className="transfer-popup" role="menu">
                            {!finished && snapshot.state === "paused" && (
                                <button
                                    type="button"
                                    role="menuitem"
                                    onClick={() => run(resumeTransfer(view.id))}
                                >
                                    Resume
                                </button>
                            )}
                            {!finished && snapshot.state !== "paused" && (
                                <button
                                    type="button"
                                    role="menuitem"
                                    onClick={() => run(pauseTransfer(view.id))}
                                >
                                    Pause
                                </button>
                            )}
                            {!finished && (
                                <button
                                    type="button"
                                    role="menuitem"
                                    onClick={() => run(cancelTransfer(view.id))}
                                >
                                    {snapshot.mode === "delete"
                                        ? "Stop deleting"
                                        : "Cancel transfer"}
                                </button>
                            )}
                            {finished && (
                                <button
                                    type="button"
                                    role="menuitem"
                                    onClick={() =>
                                        run(
                                            dismissTransfer(view.id).then(() =>
                                                onDismissed(view.id),
                                            ),
                                        )
                                    }
                                >
                                    Remove from list
                                </button>
                            )}
                        </div>
                    )}
                </div>
            </header>

            <h2 className="transfer-title">{title}</h2>
            <hr className="transfer-rule" />

            {snapshot.conflict ? (
                <ConflictPrompt
                    view={view}
                    onAnswer={(resolution, all) =>
                        run(answerTransferConflict(view.id, resolution, all))
                    }
                />
            ) : (
                <p className="transfer-status" aria-live="polite">
                    {statusLine(view)}
                </p>
            )}

            <div className="transfer-figure">
                <span className="transfer-percent">{percent}%</span>
                <span className={`transfer-badge ${state.tone}`}>
                    {state.text}
                </span>
                {state.note && (
                    <span className="transfer-note">{state.note}</span>
                )}
            </div>

            <div
                className="transfer-bar"
                role="progressbar"
                aria-label={`${title} progress`}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={percent}
            >
                {segmentFills(percent).map((fill, index) => (
                    <span
                        key={index}
                        className="transfer-segment"
                        style={
                            fill > 0 && fill < 1
                                ? {
                                      background: `linear-gradient(to right, var(--segment-on) ${fill * 100}%, var(--segment-off) ${fill * 100}%)`,
                                  }
                                : undefined
                        }
                        data-on={fill >= 1 ? "true" : "false"}
                    />
                ))}
            </div>

            {!finished && snapshot.current.length > 0 && (
                <p className="transfer-current" title={snapshot.current[0]}>
                    {snapshot.current[0]}
                    {snapshot.current.length > 1 &&
                        ` and ${snapshot.current.length - 1} more`}
                </p>
            )}

            {snapshot.failures.length > 0 && (
                <details className="transfer-problems">
                    <summary>
                        {snapshot.failures.length === 1
                            ? "1 problem"
                            : `${formatCount(snapshot.failed_files || snapshot.failures.length)} problems`}
                    </summary>
                    <ul>
                        {snapshot.failures
                            .slice(0, MAX_LISTED_FAILURES)
                            .map((failure, index) => (
                                <li key={`${failure.name}-${index}`}>
                                    <strong>{failure.name}</strong>
                                    <span>{describeFailure(failure)}</span>
                                </li>
                            ))}
                    </ul>
                </details>
            )}
        </article>
    );
}

function upsert(views: TransferView[], next: TransferView): TransferView[] {
    const others = views.filter((view) => view.id !== next.id);
    return [...others, next].sort((a, b) => b.id - a.id);
}

export function TransferWindow() {
    const [views, setViews] = useState<TransferView[]>([]);
    const [error, setError] = useState<string | null>(null);

    useEffect(() => {
        let stopped = false;
        let unsubscribe: (() => void) | undefined;
        subscribeTransfers((view) =>
            setViews((current) => upsert(current, view)),
        )
            .then((stop) => {
                if (stopped) stop();
                else unsubscribe = stop;
            })
            .catch(() => setError("Live progress is unavailable."));
        listTransfers()
            .then((initial) =>
                setViews((current) =>
                    initial.reduce(
                        (all, view) =>
                            all.some((existing) => existing.id === view.id)
                                ? all
                                : upsert(all, view),
                        current,
                    ),
                ),
            )
            .catch(() => setError("Unable to load transfers."));
        return () => {
            stopped = true;
            unsubscribe?.();
        };
    }, []);

    const remove = useCallback(
        (id: number) =>
            setViews((current) => current.filter((view) => view.id !== id)),
        [],
    );

    return (
        <main className="transfer-window">
            {error && (
                <p className="transfer-error" role="alert">
                    {error}
                </p>
            )}
            {views.length === 0 ? (
                <p className="transfer-empty">
                    No transfers yet. Copy or cut files, then right-click a
                    folder and choose “Paste with Bifrost Drive”.
                </p>
            ) : (
                views.map((view) => (
                    <TransferCard
                        key={view.id}
                        view={view}
                        onError={setError}
                        onDismissed={remove}
                    />
                ))
            )}
        </main>
    );
}
