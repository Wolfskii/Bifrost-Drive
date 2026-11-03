import { ArrowLeftRight, Download, Trash2, Upload } from "lucide-react";
import type { TransferHistoryEntry } from "./api";
import {
    describeFailure,
    entryPercent,
    formatCount,
    formatDuration,
    formatElapsed,
    formatSpeed,
    formatWhen,
    historyDetail,
    historyStatus,
    historyTitle,
} from "./transferFormat";
import "./TransferLog.css";

function DirectionIcon({ entry }: { entry: TransferHistoryEntry }) {
    if (entry.mode === "delete") return <Trash2 size={17} />;
    if (entry.direction === "upload") return <Upload size={17} />;
    if (entry.direction === "download") return <Download size={17} />;
    return <ArrowLeftRight size={17} />;
}

function Progress({ entry }: { entry: TransferHistoryEntry }) {
    const live = entry.live;
    if (!live) return null;
    const percent = entryPercent(entry);
    const speed =
        entry.mode === "delete"
            ? `${formatCount(live.files_per_second)} files/s`
            : formatSpeed(live.bytes_per_second);
    return (
        <div className="transfer-log-progress">
            <div
                className="transfer-log-bar"
                role="progressbar"
                aria-label={`${historyTitle(entry)} progress`}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={percent}
            >
                <span style={{ width: `${percent}%` }} />
            </div>
            <span className="transfer-log-percent">
                {percent}% · {speed}
                {live.seconds_left !== null &&
                    ` · ${formatDuration(live.seconds_left)} left`}
            </span>
            {live.current.length > 0 && (
                <span className="transfer-log-current" title={live.current[0]}>
                    {live.current[0]}
                    {live.current.length > 1 &&
                        ` and ${live.current.length - 1} more`}
                </span>
            )}
        </div>
    );
}

function Row({ entry }: { entry: TransferHistoryEntry }) {
    const status = historyStatus(entry.state);
    const title = historyTitle(entry);
    const hidden = Math.max(0, entry.item_count - entry.items.length);
    const duration =
        entry.finished_at !== null
            ? formatElapsed(
                  Date.parse(entry.finished_at) - Date.parse(entry.started_at),
              )
            : null;
    return (
        <article className={`transfer-log-row tone-${status.tone}`}>
            <span className="transfer-log-icon">
                <DirectionIcon entry={entry} />
            </span>
            <div className="transfer-log-body">
                <div className="transfer-log-head">
                    <strong title={title}>{title}</strong>
                    <span className={`transfer-log-status tone-${status.tone}`}>
                        {status.label}
                    </span>
                </div>
                <p className="transfer-log-detail">
                    {historyDetail(entry)}
                    <time dateTime={entry.started_at}>
                        {formatWhen(entry.started_at)}
                        {duration ? ` · took ${duration}` : ""}
                    </time>
                </p>
                <Progress entry={entry} />
                {entry.items.length > 0 && (
                    <details className="transfer-log-more">
                        <summary>
                            {entry.mode === "delete" ? "Deleted" : "Selected"}{" "}
                            {entry.item_count === 1
                                ? "1 item"
                                : `${formatCount(entry.item_count)} items`}
                        </summary>
                        <ul>
                            {entry.items.map((item) => (
                                <li key={item}>{item}</li>
                            ))}
                            {hidden > 0 && (
                                <li>…and {formatCount(hidden)} more</li>
                            )}
                        </ul>
                    </details>
                )}
                {entry.failures.length > 0 && (
                    <details className="transfer-log-more problems" open>
                        <summary>
                            {entry.failures.length === 1
                                ? "1 problem"
                                : `${formatCount(entry.failures.length)} problems`}
                            {entry.failed_files > entry.failures.length &&
                                ` (first ${formatCount(entry.failures.length)} of ${formatCount(entry.failed_files)} shown)`}
                        </summary>
                        <ul>
                            {entry.failures.map((failure, index) => (
                                <li key={`${failure.name}-${index}`}>
                                    {failure.name && (
                                        <strong>{failure.name}</strong>
                                    )}
                                    <span>{describeFailure(failure)}</span>
                                </li>
                            ))}
                        </ul>
                    </details>
                )}
            </div>
        </article>
    );
}

/** The log of Explorer transfers: what is running, and what ran, with how it went. */
export function TransferLog({
    entries,
    onClear,
    onOpenWindow,
}: {
    entries: TransferHistoryEntry[];
    onClear: () => void;
    onOpenWindow: () => void;
}) {
    const running = entries.some((entry) => entry.live !== null);
    const finished = entries.some((entry) => entry.live === null);
    return (
        <section
            className="file-browser"
            id="transfers"
            aria-labelledby="transfers-title"
        >
            <div className="section-heading">
                <div>
                    <p className="eyebrow">Copies, moves and deletes</p>
                    <h2 id="transfers-title">Transfers</h2>
                </div>
                <div className="transfer-log-actions">
                    {running && (
                        <button
                            className="secondary-button"
                            type="button"
                            onClick={onOpenWindow}
                        >
                            Show transfer window
                        </button>
                    )}
                    {finished && (
                        <button
                            className="secondary-button"
                            type="button"
                            onClick={onClear}
                        >
                            Clear finished
                        </button>
                    )}
                </div>
            </div>
            {entries.length === 0 ? (
                <p className="empty-files">
                    No transfers yet. Copy or cut files in Explorer, then choose
                    “Paste with Bifrost Drive” in a folder’s context menu.
                </p>
            ) : (
                <div className="transfer-log-list">
                    {entries.map((entry) => (
                        <Row key={entry.id} entry={entry} />
                    ))}
                </div>
            )}
        </section>
    );
}
