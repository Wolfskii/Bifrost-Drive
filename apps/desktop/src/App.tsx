import {
    Cloud,
    HardDrive,
    Settings,
    Activity,
    Plus,
    ArrowUpRight,
    File,
    Folder,
    RefreshCw,
} from "lucide-react";
import { FormEvent, useEffect, useState } from "react";
import {
    ActivitySummary,
    ConnectionSummary,
    ConflictSummary,
    createS3Connection,
    createSftpConnection,
    createWebDavConnection,
    FileSummary,
    hydrateFile,
    listConnections,
    listActivity,
    listConflicts,
    listFiles,
    resolveConflict,
    runSync,
    S3ConnectionForm,
} from "./api";

const providerTypes = [
    { name: "S3", status: "Available now" },
    { name: "SFTP", status: "Password sign-in available" },
    { name: "WebDAV", status: "Available now" },
];

export function App() {
    const [connections, setConnections] = useState<ConnectionSummary[]>([]);
    const [conflicts, setConflicts] = useState<ConflictSummary[]>([]);
    const [activity, setActivity] = useState<ActivitySummary[]>([]);
    const [files, setFiles] = useState<FileSummary[]>([]);
    const [openedConnection, setOpenedConnection] =
        useState<ConnectionSummary | null>(null);
    const [loadingFiles, setLoadingFiles] = useState(false);
    const [wizardOpen, setWizardOpen] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [saving, setSaving] = useState(false);
    const [hydratingPath, setHydratingPath] = useState<string | null>(null);
    const [providerChoice, setProviderChoice] = useState<
        "S3" | "SFTP" | "WebDAV"
    >("S3");

    useEffect(() => {
        listConnections()
            .then(setConnections)
            .catch(() => {
                setError("Unable to load saved connections.");
            });
    }, []);

    useEffect(() => {
        listActivity()
            .then(setActivity)
            .catch(() => {
                setError("Unable to load activity history.");
            });
    }, []);

    useEffect(() => {
        listConflicts()
            .then(setConflicts)
            .catch(() => {
                setError("Unable to load unresolved conflicts.");
            });
    }, []);

    async function handleOpen(connection: ConnectionSummary) {
        setLoadingFiles(true);
        setError(null);
        try {
            setFiles(await listFiles(connection.id));
            setOpenedConnection(connection);
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to list remote files.",
            );
        } finally {
            setLoadingFiles(false);
        }
    }

    async function handleCreate(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        const values = new FormData(event.currentTarget);
        const name = String(values.get("name") ?? "").trim();
        const common = {
            name,
            username: String(values.get("username") ?? "").trim(),
            password: String(values.get("password") ?? ""),
        };
        let createConnection: Promise<ConnectionSummary>;
        if (providerChoice === "WebDAV") {
            createConnection = createWebDavConnection({
                ...common,
                endpoint: String(values.get("endpoint") ?? "").trim(),
            });
        } else if (providerChoice === "SFTP") {
            createConnection = createSftpConnection({
                ...common,
                host: String(values.get("host") ?? "").trim(),
                port: Number(values.get("port") ?? 22),
                knownHosts: String(values.get("knownHosts") ?? "").trim(),
                authentication: String(
                    values.get("authentication") ?? "password",
                ) as "password" | "private_key",
                privateKeyPath: String(
                    values.get("privateKeyPath") ?? "",
                ).trim(),
                passphrase: String(values.get("passphrase") ?? ""),
            });
        } else {
            const form: S3ConnectionForm = {
                name,
                endpoint: String(values.get("endpoint") ?? "").trim(),
                region: String(values.get("region") ?? "").trim(),
                bucket: String(values.get("bucket") ?? "").trim(),
                pathStyle: values.get("pathStyle") === "on",
                accessKeyId: String(values.get("accessKeyId") ?? "").trim(),
                secretAccessKey: String(values.get("secretAccessKey") ?? ""),
            };
            createConnection = createS3Connection(form);
        }
        setSaving(true);
        setError(null);
        try {
            const connection = await createConnection;
            setConnections((current) => [...current, connection]);
            setWizardOpen(false);
            event.currentTarget.reset();
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to create connection.",
            );
        } finally {
            setSaving(false);
        }
    }

    async function handleHydrate(path: string) {
        if (!openedConnection) return;
        setHydratingPath(path);
        setError(null);
        try {
            await hydrateFile(openedConnection.id, path);
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to download the file.",
            );
        } finally {
            setHydratingPath(null);
        }
    }

    async function handleSync(path: string) {
        if (!openedConnection) return;
        setHydratingPath(path);
        setError(null);
        try {
            const result = await runSync(openedConnection.id, path);
            if (result.conflict) {
                setConflicts(await listConflicts());
            }
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to synchronize the file.",
            );
        } finally {
            setHydratingPath(null);
        }
    }

    async function handleResolveConflict(
        conflict: ConflictSummary,
        resolution: "keep_local" | "keep_remote",
    ) {
        setError(null);
        try {
            await resolveConflict(conflict.id, resolution);
            setConflicts((current) =>
                current.filter((item) => item.id !== conflict.id),
            );
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to resolve the conflict.",
            );
        }
    }

    return (
        <main className="app-shell">
            <aside className="sidebar">
                <div className="brand-lockup">
                    <div className="brand-mark" aria-hidden="true">
                        <Cloud size={18} strokeWidth={2.5} />
                    </div>
                    <div>
                        <strong>Bifrost Drive</strong>
                        <span>One gateway. Every storage.</span>
                    </div>
                </div>
                <nav aria-label="Primary navigation">
                    <a className="nav-item active" href="#connections">
                        <HardDrive size={17} /> Connections
                    </a>
                    <a className="nav-item" href="#activity">
                        <Activity size={17} /> Activity
                    </a>
                    <a className="nav-item" href="#settings">
                        <Settings size={17} /> Settings
                    </a>
                </nav>
                <div className="sidebar-footer">
                    <span className="status-dot" /> Service ready
                    <small>Foundation build 0.1.0</small>
                </div>
            </aside>
            <section className="content" id="connections">
                <header className="topbar">
                    <div>
                        <p className="eyebrow">Storage workspace</p>
                        <h1>Your connections</h1>
                        <p className="lede">
                            Remote files, ready when you are.
                        </p>
                    </div>
                    <button
                        className="primary-button"
                        type="button"
                        onClick={() => setWizardOpen(true)}
                    >
                        <Plus size={17} /> Add connection
                    </button>
                </header>
                {error && (
                    <p className="inline-error" role="alert">
                        {error}
                    </p>
                )}
                {conflicts.length > 0 && (
                    <section
                        className="saved-connections"
                        aria-labelledby="conflicts-title"
                    >
                        <div className="section-heading">
                            <div>
                                <p className="eyebrow">Needs attention</p>
                                <h2 id="conflicts-title">File conflicts</h2>
                            </div>
                            <span className="muted-label">
                                {conflicts.length} unresolved
                            </span>
                        </div>
                        <div className="connection-list">
                            {conflicts.map((conflict) => (
                                <article
                                    className="connection-row"
                                    key={conflict.id}
                                >
                                    <div className="provider-icon">
                                        <Activity size={20} />
                                    </div>
                                    <div>
                                        <h3>{conflict.remote_path}</h3>
                                        <p>Local and remote changes differ.</p>
                                    </div>
                                    <button
                                        className="link-button"
                                        type="button"
                                        onClick={() =>
                                            handleResolveConflict(
                                                conflict,
                                                "keep_remote",
                                            )
                                        }
                                    >
                                        Keep remote
                                    </button>
                                    <button
                                        className="link-button"
                                        type="button"
                                        onClick={() =>
                                            handleResolveConflict(
                                                conflict,
                                                "keep_local",
                                            )
                                        }
                                    >
                                        Keep local
                                    </button>
                                </article>
                            ))}
                        </div>
                    </section>
                )}
                {connections.length > 0 && (
                    <section
                        className="saved-connections"
                        aria-labelledby="saved-title"
                    >
                        <div className="section-heading">
                            <div>
                                <p className="eyebrow">Connected storage</p>
                                <h2 id="saved-title">Your spaces</h2>
                            </div>
                            <span className="muted-label">
                                {connections.length} saved
                            </span>
                        </div>
                        <div className="connection-list">
                            {connections.map((connection) => (
                                <article
                                    className="connection-row"
                                    key={connection.id}
                                >
                                    <div className="provider-icon">
                                        <Cloud size={20} />
                                    </div>
                                    <div>
                                        <h3>{connection.name}</h3>
                                        <p>{connection.endpoint}</p>
                                    </div>
                                    <span className="connection-state">
                                        <span className="status-dot" />{" "}
                                        {connection.state}
                                    </span>
                                    <button
                                        className="link-button"
                                        type="button"
                                        onClick={() => handleOpen(connection)}
                                        disabled={loadingFiles}
                                    >
                                        {loadingFiles ? "Loading..." : "Open"}
                                    </button>
                                </article>
                            ))}
                        </div>
                    </section>
                )}
                {openedConnection && (
                    <section
                        className="file-browser"
                        aria-labelledby="files-title"
                    >
                        <div className="section-heading">
                            <div>
                                <p className="eyebrow">Remote files</p>
                                <h2 id="files-title">
                                    {openedConnection.name}
                                </h2>
                            </div>
                            <button
                                className="icon-button"
                                type="button"
                                aria-label="Refresh files"
                                onClick={() => handleOpen(openedConnection)}
                            >
                                <RefreshCw size={16} />
                            </button>
                        </div>
                        {files.length === 0 ? (
                            <p className="empty-files">
                                This space has no items at its root.
                            </p>
                        ) : (
                            <div className="file-list">
                                {files.map((file) => (
                                    <div className="file-row" key={file.path}>
                                        <span className="file-icon">
                                            {file.is_directory ? (
                                                <Folder size={17} />
                                            ) : (
                                                <File size={17} />
                                            )}
                                        </span>
                                        <span className="file-name">
                                            {file.path}
                                        </span>
                                        <span className="file-size">
                                            {file.is_directory
                                                ? "Folder"
                                                : formatBytes(file.size_bytes)}
                                        </span>
                                        {openedConnection.kind === "S3" &&
                                            !file.is_directory && (
                                                <button
                                                    className="link-button"
                                                    type="button"
                                                    onClick={() =>
                                                        handleHydrate(file.path)
                                                    }
                                                    disabled={
                                                        hydratingPath ===
                                                        file.path
                                                    }
                                                >
                                                    {hydratingPath === file.path
                                                        ? "Downloading..."
                                                        : "Download"}
                                                </button>
                                            )}
                                        {!file.is_directory && (
                                            <button
                                                className="link-button"
                                                type="button"
                                                onClick={() =>
                                                    handleSync(file.path)
                                                }
                                                disabled={
                                                    hydratingPath === file.path
                                                }
                                            >
                                                {hydratingPath === file.path
                                                    ? "Syncing..."
                                                    : "Sync"}
                                            </button>
                                        )}
                                    </div>
                                ))}
                            </div>
                        )}
                    </section>
                )}
                <section
                    className="file-browser"
                    id="activity"
                    aria-labelledby="activity-title"
                >
                    <div className="section-heading">
                        <div>
                            <p className="eyebrow">Recent work</p>
                            <h2 id="activity-title">Activity</h2>
                        </div>
                        <span className="muted-label">Last 100 events</span>
                    </div>
                    {activity.length === 0 ? (
                        <p className="empty-files">No transfer activity yet.</p>
                    ) : (
                        <div className="file-list">
                            {activity.map((event) => (
                                <div className="file-row" key={event.id}>
                                    <span className="file-icon">
                                        <Activity size={17} />
                                    </span>
                                    <span className="file-name">
                                        {event.kind} {event.remote_path ?? ""}
                                    </span>
                                    <span className="file-size">
                                        {event.status}
                                    </span>
                                </div>
                            ))}
                        </div>
                    )}
                </section>
                <section
                    className="welcome-panel"
                    aria-labelledby="welcome-title"
                >
                    <div>
                        <span className="panel-kicker">
                            Early access foundation
                        </span>
                        <h2 id="welcome-title">
                            A quieter way to reach every file.
                        </h2>
                        <p>
                            Connect an S3-compatible space, keep its credentials
                            in Windows Credential Manager, and browse remote
                            metadata without downloading every file.
                        </p>
                    </div>
                    <div className="bridge-graphic" aria-hidden="true">
                        <span />
                        <span />
                        <span />
                    </div>
                </section>
                <section className="section-heading">
                    <div>
                        <p className="eyebrow">Provider support</p>
                        <h2>Connection types</h2>
                    </div>
                    <span className="muted-label">
                        {
                            providerTypes.filter(
                                (provider) =>
                                    provider.status === "Available now",
                            ).length
                        }{" "}
                        connection flow ready
                    </span>
                </section>
                <div className="provider-grid">
                    {providerTypes.map((provider) => (
                        <article className="provider-card" key={provider.name}>
                            <div className="provider-icon">
                                <Cloud size={20} />
                            </div>
                            <div>
                                <h3>{provider.name}</h3>
                                <p>{provider.status}</p>
                            </div>
                            <ArrowUpRight className="card-arrow" size={17} />
                        </article>
                    ))}
                </div>
            </section>
            {wizardOpen && (
                <div className="modal-backdrop" role="presentation">
                    <section
                        className="wizard"
                        role="dialog"
                        aria-modal="true"
                        aria-labelledby="wizard-title"
                    >
                        <div className="wizard-header">
                            <div>
                                <p className="eyebrow">New connection</p>
                                <h2 id="wizard-title">
                                    Connect to {providerChoice}
                                </h2>
                            </div>
                            <button
                                className="icon-button"
                                type="button"
                                aria-label="Close"
                                onClick={() => setWizardOpen(false)}
                            >
                                ×
                            </button>
                        </div>
                        <p className="wizard-copy">
                            Credentials are stored in Windows Credential Manager
                            and never in the app database.
                        </p>
                        <form onSubmit={handleCreate}>
                            <label>
                                Storage type
                                <select
                                    value={providerChoice}
                                    onChange={(event) =>
                                        setProviderChoice(
                                            event.target.value as
                                                "S3" | "SFTP" | "WebDAV",
                                        )
                                    }
                                >
                                    <option value="S3">
                                        S3-compatible storage
                                    </option>
                                    <option value="SFTP">SFTP server</option>
                                    <option value="WebDAV">
                                        WebDAV server
                                    </option>
                                </select>
                            </label>
                            <label>
                                Connection name
                                <input
                                    name="name"
                                    required
                                    placeholder="Production S3"
                                />
                            </label>
                            {providerChoice !== "SFTP" && (
                                <label>
                                    Endpoint
                                    <input
                                        name="endpoint"
                                        type="url"
                                        required
                                        defaultValue="https://s3.amazonaws.com"
                                    />
                                </label>
                            )}
                            {providerChoice === "SFTP" && (
                                <label>
                                    Host
                                    <input
                                        name="host"
                                        required
                                        placeholder="files.example.com"
                                    />
                                </label>
                            )}
                            {providerChoice === "SFTP" && (
                                <div className="form-grid">
                                    <label>
                                        Port
                                        <input
                                            name="port"
                                            type="number"
                                            min="1"
                                            max="65535"
                                            defaultValue="22"
                                            required
                                        />
                                    </label>
                                    <label>
                                        Known hosts path
                                        <input
                                            name="knownHosts"
                                            required
                                            placeholder="C:\\Users\\you\\.ssh\\known_hosts"
                                        />
                                    </label>
                                </div>
                            )}
                            {providerChoice === "SFTP" && (
                                <>
                                    <label>
                                        Authentication
                                        <select
                                            name="authentication"
                                            defaultValue="password"
                                        >
                                            <option value="password">
                                                Password
                                            </option>
                                            <option value="private_key">
                                                Private key
                                            </option>
                                        </select>
                                    </label>
                                    <div className="form-grid">
                                        <label>
                                            Private key path
                                            <input
                                                name="privateKeyPath"
                                                placeholder="C:\\Users\\you\\.ssh\\id_ed25519"
                                            />
                                        </label>
                                        <label>
                                            Key passphrase
                                            <input
                                                name="passphrase"
                                                type="password"
                                                autoComplete="new-password"
                                            />
                                        </label>
                                    </div>
                                </>
                            )}
                            {providerChoice !== "S3" && (
                                <div className="form-grid">
                                    <label>
                                        Username
                                        <input
                                            name="username"
                                            required
                                            autoComplete="username"
                                        />
                                    </label>
                                    <label>
                                        Password
                                        <input
                                            name="password"
                                            required
                                            type="password"
                                            autoComplete="current-password"
                                        />
                                    </label>
                                </div>
                            )}
                            {providerChoice === "S3" && (
                                <>
                                    <div className="form-grid">
                                        <label>
                                            Bucket
                                            <input
                                                name="bucket"
                                                required
                                                placeholder="company-data"
                                            />
                                        </label>
                                        <label>
                                            Region
                                            <input
                                                name="region"
                                                required
                                                defaultValue="us-east-1"
                                            />
                                        </label>
                                    </div>
                                    <div className="form-grid">
                                        <label>
                                            Access key ID
                                            <input
                                                name="accessKeyId"
                                                required
                                                autoComplete="off"
                                            />
                                        </label>
                                        <label>
                                            Secret access key
                                            <input
                                                name="secretAccessKey"
                                                required
                                                type="password"
                                                autoComplete="new-password"
                                            />
                                        </label>
                                    </div>
                                </>
                            )}
                            {providerChoice === "S3" && (
                                <label className="checkbox-row">
                                    <input name="pathStyle" type="checkbox" />{" "}
                                    Use path-style addressing
                                </label>
                            )}
                            <div className="wizard-actions">
                                <button
                                    className="secondary-button"
                                    type="button"
                                    onClick={() => setWizardOpen(false)}
                                >
                                    Cancel
                                </button>
                                <button
                                    className="primary-button"
                                    disabled={saving}
                                    type="submit"
                                >
                                    {saving ? "Connecting..." : "Test and save"}
                                </button>
                            </div>
                        </form>
                    </section>
                </div>
            )}
        </main>
    );
}

function formatBytes(value: number | null): string {
    if (value === null) return "Size unavailable";
    if (value < 1024) return `${value} B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
    if (value < 1024 * 1024 * 1024)
        return `${(value / (1024 * 1024)).toFixed(1)} MB`;
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}
