import {
    Cloud,
    HardDrive,
    Settings,
    Activity,
    Plus,
    Pencil,
    ArrowUpRight,
    File,
    Folder,
    RefreshCw,
    Trash2,
} from "lucide-react";
import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
} from "@tauri-apps/plugin-notification";
import { FormEvent, useEffect, useState } from "react";
import {
    ActivitySummary,
    ConnectionSummary,
    ConflictSummary,
    createFtpConnection,
    createS3Connection,
    createSftpConnection,
    createSmbConnection,
    createWebDavConnection,
    checkForUpdate,
    getAutostartEnabled,
    getAvailableDriveLetters,
    getConnectionDetails,
    FileSummary,
    hydrateFile,
    installUpdate,
    listConnections,
    listActivity,
    listConflicts,
    listFiles,
    registerSyncRoot,
    removeConnection,
    resolveConflict,
    runSync,
    setAutostartEnabled,
    S3ConnectionForm,
    SyncRootRegisterResponse,
    updateConnection,
} from "./api";

const providerTypes = [
    { name: "S3", status: "Available now" },
    { name: "SFTP", status: "Password sign-in available" },
    { name: "WebDAV", status: "Available now" },
    { name: "FTP / FTPS", status: "Available now" },
    { name: "SMB", status: "Available now" },
];

type ProviderChoice = "S3" | "SFTP" | "WebDAV" | "FTP" | "SMB";

type FormDefaults = Record<string, boolean | number | string>;

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
    const [updateVersion, setUpdateVersion] = useState<string | null>(null);
    const [installingUpdate, setInstallingUpdate] = useState(false);
    const [autostartEnabled, setAutostartEnabledState] = useState(false);
    const [updatingAutostart, setUpdatingAutostart] = useState(false);
    const [sftpAuthentication, setSftpAuthentication] = useState<
        "password" | "private_key"
    >("password");
    const [explorerPaths, setExplorerPaths] = useState<Record<string, string>>(
        {},
    );
    const [driveAssignments, setDriveAssignments] = useState<
        Record<string, string>
    >({});
    const [availableDriveLetters, setAvailableDriveLetters] = useState<
        string[]
    >([]);
    const [selectedDriveLetter, setSelectedDriveLetter] = useState("");
    const [editingConnection, setEditingConnection] =
        useState<ConnectionSummary | null>(null);
    const [formDefaults, setFormDefaults] = useState<FormDefaults>({});
    const [providerChoice, setProviderChoice] = useState<ProviderChoice>("S3");

    useEffect(() => {
        listConnections()
            .then(async (loadedConnections) => {
                setConnections(loadedConnections);
                const registeredRoots = await Promise.all(
                    loadedConnections.map(async (connection) => {
                        try {
                            const root = await registerSyncRoot(connection.id);
                            return [connection.id, root] as const;
                        } catch {
                            return null;
                        }
                    }),
                );
                setExplorerPaths(
                    Object.fromEntries(
                        registeredRoots
                            .filter(
                                (
                                    root,
                                ): root is readonly [
                                    string,
                                    SyncRootRegisterResponse,
                                ] => root !== null,
                            )
                            .map(([id, root]) => [id, root.path]),
                    ),
                );
                setDriveAssignments(
                    Object.fromEntries(
                        registeredRoots
                            .filter(
                                (
                                    root,
                                ): root is readonly [
                                    string,
                                    SyncRootRegisterResponse,
                                ] =>
                                    root !== null &&
                                    root[1].drive_letter !== null,
                            )
                            .map(([id, root]) => [
                                id,
                                root.drive_letter as string,
                            ]),
                    ),
                );
            })
            .catch(() => {
                setError("Unable to load saved connections.");
            });
    }, []);

    useEffect(() => {
        getAvailableDriveLetters()
            .then((letters) => {
                setAvailableDriveLetters(letters);
                setSelectedDriveLetter(
                    (current) => current || (letters.includes("Z") ? "Z" : ""),
                );
            })
            .catch(() => undefined);
    }, []);

    useEffect(() => {
        checkForUpdate()
            .then(setUpdateVersion)
            .catch(() => undefined);
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

    useEffect(() => {
        getAutostartEnabled()
            .then(setAutostartEnabledState)
            .catch(() => undefined);
    }, []);

    async function handleAutostartChange(enabled: boolean) {
        setUpdatingAutostart(true);
        setError(null);
        try {
            await setAutostartEnabled(enabled);
            setAutostartEnabledState(enabled);
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to update startup settings.",
            );
        } finally {
            setUpdatingAutostart(false);
        }
    }

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

    async function handleEdit(connection: ConnectionSummary) {
        setError(null);
        try {
            const details = await getConnectionDetails(connection.id);
            const configuration = details.configuration;
            const sftpUrl =
                connection.kind === "Sftp"
                    ? new URL(connection.endpoint)
                    : null;
            setFormDefaults({
                name: connection.name,
                endpoint: connection.endpoint,
                username: details.username ?? "",
                host: sftpUrl?.hostname ?? "",
                port: sftpUrl?.port ? Number(sftpUrl.port) : 22,
                rootPath: String(configuration.root_path ?? ""),
                domain: String(configuration.domain ?? ""),
                bucket: String(configuration.bucket ?? ""),
                region: String(configuration.region ?? ""),
                pathStyle: Boolean(configuration.path_style),
                privateKeyPath: String(configuration.private_key_path ?? ""),
                trustOnFirstUse: Boolean(configuration.trust_on_first_use),
                knownHosts: String(configuration.known_hosts ?? ""),
            });
            const driveLetter = String(configuration.drive_letter ?? "");
            setSelectedDriveLetter(driveLetter);
            if (driveLetter) {
                setAvailableDriveLetters((current) =>
                    current.includes(driveLetter)
                        ? current
                        : [...current, driveLetter].sort(),
                );
            }
            setProviderChoice(providerChoiceFor(connection.kind));
            setSftpAuthentication(
                configuration.authentication === "private_key"
                    ? "private_key"
                    : "password",
            );
            setEditingConnection(connection);
            setWizardOpen(true);
        } catch (cause) {
            setError(errorMessage(cause, "Unable to load connection details."));
        }
    }

    async function handleRemove(connection: ConnectionSummary) {
        if (!window.confirm(`Remove connection "${connection.name}"?`)) {
            return;
        }
        setError(null);
        try {
            await removeConnection(connection.id);
            setConnections((current) =>
                current.filter((item) => item.id !== connection.id),
            );
            setExplorerPaths((current) => {
                const next = { ...current };
                delete next[connection.id];
                return next;
            });
            setDriveAssignments((current) => {
                const next = { ...current };
                delete next[connection.id];
                return next;
            });
            if (openedConnection?.id === connection.id) {
                setOpenedConnection(null);
                setFiles([]);
            }
        } catch (cause) {
            setError(errorMessage(cause, "Unable to remove connection."));
        }
    }

    async function handleCreate(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        const form = event.currentTarget;
        const values = new FormData(form);
        const name = String(values.get("name") ?? "").trim();
        const common = {
            name,
            username: String(values.get("username") ?? "").trim(),
            password: String(values.get("password") ?? ""),
            driveLetter: String(values.get("driveLetter") ?? ""),
        };
        let connectionOperation: Promise<ConnectionSummary>;
        const endpoint = String(values.get("endpoint") ?? "").trim();
        const driveLetter = String(values.get("driveLetter") ?? "");
        if (editingConnection) {
            let updateEndpoint = endpoint;
            let configuration: Record<string, unknown>;
            let credentials: Record<string, string>;
            if (providerChoice === "FTP") {
                configuration = { drive_letter: driveLetter || null };
                credentials = {
                    username: common.username,
                    password: common.password,
                };
            } else if (providerChoice === "SMB") {
                configuration = {
                    domain: String(values.get("domain") ?? "").trim(),
                    drive_letter: driveLetter || null,
                };
                credentials = {
                    username: common.username,
                    password: common.password,
                };
            } else if (providerChoice === "WebDAV") {
                configuration = { drive_letter: driveLetter || null };
                credentials = {
                    username: common.username,
                    password: common.password,
                };
            } else if (providerChoice === "SFTP") {
                const host = String(values.get("host") ?? "").trim();
                const port = Number(values.get("port") ?? 22);
                updateEndpoint = `sftp://${host}:${port}`;
                configuration = {
                    host,
                    port,
                    root_path: String(values.get("rootPath") ?? "").trim(),
                    known_hosts: formDefaults.knownHosts ?? "",
                    authentication: String(
                        values.get("authentication") ?? "password",
                    ),
                    trust_on_first_use: values.get("trustOnFirstUse") === "on",
                    private_key_path: String(
                        values.get("privateKeyPath") ?? "",
                    ).trim(),
                    drive_letter: driveLetter || null,
                };
                credentials = {
                    username: common.username,
                    password: common.password,
                    private_key_path: String(
                        values.get("privateKeyPath") ?? "",
                    ).trim(),
                    passphrase: String(values.get("passphrase") ?? ""),
                };
            } else {
                configuration = {
                    region: String(values.get("region") ?? "").trim(),
                    bucket: String(values.get("bucket") ?? "").trim(),
                    path_style: values.get("pathStyle") === "on",
                    drive_letter: driveLetter || null,
                };
                credentials = {
                    access_key_id: String(
                        values.get("accessKeyId") ?? "",
                    ).trim(),
                    secret_access_key: String(
                        values.get("secretAccessKey") ?? "",
                    ),
                };
            }
            connectionOperation = updateConnection({
                id: editingConnection.id,
                name,
                endpoint: updateEndpoint,
                configuration,
                credentials,
            });
        } else if (providerChoice === "FTP") {
            connectionOperation = createFtpConnection({
                ...common,
                endpoint: String(values.get("endpoint") ?? "").trim(),
            });
        } else if (providerChoice === "SMB") {
            connectionOperation = createSmbConnection({
                ...common,
                endpoint: String(values.get("endpoint") ?? "").trim(),
                domain: String(values.get("domain") ?? "").trim(),
            });
        } else if (providerChoice === "WebDAV") {
            connectionOperation = createWebDavConnection({
                ...common,
                endpoint: String(values.get("endpoint") ?? "").trim(),
            });
        } else if (providerChoice === "SFTP") {
            connectionOperation = createSftpConnection({
                ...common,
                host: String(values.get("host") ?? "").trim(),
                port: Number(values.get("port") ?? 22),
                rootPath: String(values.get("rootPath") ?? "").trim(),
                authentication: String(
                    values.get("authentication") ?? "password",
                ) as "password" | "private_key",
                trustOnFirstUse: values.get("trustOnFirstUse") === "on",
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
                driveLetter: String(values.get("driveLetter") ?? ""),
            };
            connectionOperation = createS3Connection(form);
        }
        setSaving(true);
        setError(null);
        try {
            const connection = await connectionOperation;
            setConnections((current) => [
                ...current.filter((item) => item.id !== editingConnection?.id),
                connection,
            ]);
            try {
                const root = await registerSyncRoot(
                    connection.id,
                    String(values.get("driveLetter") ?? ""),
                );
                setExplorerPaths((current) => ({
                    ...current,
                    [connection.id]: root.path,
                }));
                if (root.drive_letter) {
                    setDriveAssignments((current) => ({
                        ...current,
                        [connection.id]: root.drive_letter as string,
                    }));
                }
                const remainingLetters = await getAvailableDriveLetters();
                setAvailableDriveLetters(remainingLetters);
            } catch (cause) {
                setError(
                    `Connection saved, but it could not be registered in Explorer: ${errorMessage(cause, "unknown error")}`,
                );
            }
            setWizardOpen(false);
            form.reset();
            setEditingConnection(null);
            setFormDefaults({});
            setSftpAuthentication("password");
            setSelectedDriveLetter("");
        } catch (cause) {
            setError(errorMessage(cause, "Unable to save connection."));
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
            await notify("File ready", path);
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
            } else {
                await notify("Sync complete", path);
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
            await notify("Conflict resolved", conflict.remote_path);
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
                        onClick={() => {
                            setError(null);
                            setEditingConnection(null);
                            setFormDefaults({});
                            setSftpAuthentication("password");
                            setSelectedDriveLetter(
                                availableDriveLetters.includes("Z") ? "Z" : "",
                            );
                            setWizardOpen(true);
                        }}
                    >
                        <Plus size={17} /> Add connection
                    </button>
                </header>
                {error && !wizardOpen && (
                    <p className="inline-error" role="alert">
                        {error}
                    </p>
                )}
                {updateVersion && (
                    <p className="inline-error" role="status">
                        Version {updateVersion} is available.{" "}
                        <button
                            className="link-button"
                            type="button"
                            disabled={installingUpdate}
                            onClick={async () => {
                                setInstallingUpdate(true);
                                try {
                                    await installUpdate();
                                } catch (cause) {
                                    setError(
                                        cause instanceof Error
                                            ? cause.message
                                            : "Unable to install the update.",
                                    );
                                } finally {
                                    setInstallingUpdate(false);
                                }
                            }}
                        >
                            {installingUpdate
                                ? "Installing..."
                                : "Install update"}
                        </button>
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
                                        {driveAssignments[connection.id] && (
                                            <p className="explorer-location">
                                                Drive:{" "}
                                                {
                                                    driveAssignments[
                                                        connection.id
                                                    ]
                                                }
                                                :\
                                            </p>
                                        )}
                                        {explorerPaths[connection.id] && (
                                            <p className="explorer-location">
                                                Explorer:{" "}
                                                {explorerPaths[connection.id]}
                                            </p>
                                        )}
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
                                    <button
                                        className="icon-button"
                                        type="button"
                                        aria-label={`Edit ${connection.name}`}
                                        onClick={() => handleEdit(connection)}
                                    >
                                        <Pencil size={15} />
                                    </button>
                                    <button
                                        className="icon-button"
                                        type="button"
                                        aria-label={`Remove ${connection.name}`}
                                        onClick={() => handleRemove(connection)}
                                    >
                                        <Trash2 size={15} />
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
                <section
                    className="file-browser"
                    id="settings"
                    aria-labelledby="settings-title"
                >
                    <div className="section-heading">
                        <div>
                            <p className="eyebrow">Application</p>
                            <h2 id="settings-title">Settings</h2>
                        </div>
                    </div>
                    <label className="checkbox-row">
                        <input
                            type="checkbox"
                            checked={autostartEnabled}
                            disabled={updatingAutostart}
                            onChange={(event) =>
                                handleAutostartChange(event.target.checked)
                            }
                        />
                        Start Bifrost Drive when I sign in
                    </label>
                </section>
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
                                <p className="eyebrow">
                                    {editingConnection
                                        ? "Edit connection"
                                        : "New connection"}
                                </p>
                                <h2 id="wizard-title">
                                    {editingConnection ? "Edit" : "Connect to"}{" "}
                                    {providerChoice}
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
                        {error && (
                            <p
                                className="inline-error wizard-error"
                                role="alert"
                            >
                                {error}
                            </p>
                        )}
                        <form onSubmit={handleCreate}>
                            <label>
                                Storage type
                                <select
                                    value={providerChoice}
                                    onChange={(event) =>
                                        setProviderChoice(
                                            event.target
                                                .value as ProviderChoice,
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
                                    <option value="FTP">
                                        FTP / FTPS server
                                    </option>
                                    <option value="SMB">SMB share</option>
                                </select>
                            </label>
                            <label>
                                Connection name
                                <input
                                    name="name"
                                    required
                                    defaultValue={formDefaults.name as string}
                                    placeholder="Production S3"
                                />
                            </label>
                            <label>
                                Explorer drive letter
                                <select
                                    name="driveLetter"
                                    value={selectedDriveLetter}
                                    onChange={(event) =>
                                        setSelectedDriveLetter(
                                            event.target.value,
                                        )
                                    }
                                >
                                    <option value="">Folder only</option>
                                    {availableDriveLetters.map((letter) => (
                                        <option value={letter} key={letter}>
                                            {letter}:
                                        </option>
                                    ))}
                                </select>
                            </label>
                            {providerChoice !== "SFTP" && (
                                <label>
                                    Endpoint
                                    <input
                                        name="endpoint"
                                        type="url"
                                        required
                                        defaultValue={
                                            (formDefaults.endpoint as string) ??
                                            "https://s3.amazonaws.com"
                                        }
                                    />
                                </label>
                            )}
                            {providerChoice === "SFTP" && (
                                <label>
                                    Host
                                    <input
                                        name="host"
                                        required
                                        defaultValue={
                                            formDefaults.host as string
                                        }
                                        placeholder="files.example.com"
                                    />
                                </label>
                            )}
                            {providerChoice === "SMB" && (
                                <label>
                                    Domain
                                    <input
                                        name="domain"
                                        defaultValue={
                                            formDefaults.domain as string
                                        }
                                        placeholder="WORKGROUP"
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
                                            defaultValue={
                                                (formDefaults.port as number) ??
                                                22
                                            }
                                            required
                                        />
                                    </label>
                                    <label>
                                        Start path
                                        <input
                                            name="rootPath"
                                            defaultValue={
                                                formDefaults.rootPath as string
                                            }
                                            placeholder="documents/projects"
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
                                            value={sftpAuthentication}
                                            onChange={(event) =>
                                                setSftpAuthentication(
                                                    event.target.value as
                                                        | "password"
                                                        | "private_key",
                                                )
                                            }
                                        >
                                            <option value="password">
                                                Password
                                            </option>
                                            <option value="private_key">
                                                Private key
                                            </option>
                                        </select>
                                    </label>
                                    {sftpAuthentication === "private_key" && (
                                        <div className="form-grid">
                                            <label>
                                                Private key path
                                                <input
                                                    name="privateKeyPath"
                                                    required
                                                    defaultValue={
                                                        formDefaults.privateKeyPath as string
                                                    }
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
                                    )}
                                    <label className="checkbox-row">
                                        <input
                                            name="trustOnFirstUse"
                                            type="checkbox"
                                            defaultChecked={Boolean(
                                                formDefaults.trustOnFirstUse,
                                            )}
                                        />
                                        Trust a new server key on first use
                                    </label>
                                </>
                            )}
                            {providerChoice !== "S3" && (
                                <div className="form-grid">
                                    <label>
                                        Username
                                        <input
                                            name="username"
                                            required
                                            defaultValue={
                                                formDefaults.username as string
                                            }
                                            autoComplete="username"
                                        />
                                    </label>
                                    {(providerChoice !== "SFTP" ||
                                        sftpAuthentication === "password") && (
                                        <label>
                                            Password
                                            <input
                                                name="password"
                                                required={!editingConnection}
                                                type="password"
                                                placeholder={
                                                    editingConnection
                                                        ? "Leave blank to keep current password"
                                                        : undefined
                                                }
                                                autoComplete="current-password"
                                            />
                                        </label>
                                    )}
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
                                                defaultValue={
                                                    formDefaults.bucket as string
                                                }
                                                placeholder="company-data"
                                            />
                                        </label>
                                        <label>
                                            Region
                                            <input
                                                name="region"
                                                required
                                                defaultValue={
                                                    (formDefaults.region as string) ??
                                                    "us-east-1"
                                                }
                                            />
                                        </label>
                                    </div>
                                    <div className="form-grid">
                                        <label>
                                            Access key ID
                                            <input
                                                name="accessKeyId"
                                                required={!editingConnection}
                                                autoComplete="off"
                                            />
                                        </label>
                                        <label>
                                            Secret access key
                                            <input
                                                name="secretAccessKey"
                                                required={!editingConnection}
                                                type="password"
                                                placeholder={
                                                    editingConnection
                                                        ? "Leave blank to keep current key"
                                                        : undefined
                                                }
                                                autoComplete="new-password"
                                            />
                                        </label>
                                    </div>
                                </>
                            )}
                            {providerChoice === "S3" && (
                                <label className="checkbox-row">
                                    <input
                                        name="pathStyle"
                                        type="checkbox"
                                        defaultChecked={Boolean(
                                            formDefaults.pathStyle,
                                        )}
                                    />{" "}
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
                                    {saving
                                        ? "Connecting..."
                                        : editingConnection
                                          ? "Test and save changes"
                                          : "Test and save"}
                                </button>
                            </div>
                        </form>
                    </section>
                </div>
            )}
        </main>
    );
}

async function notify(title: string, body: string): Promise<void> {
    try {
        if (!(await isPermissionGranted())) {
            const permission = await requestPermission();
            if (permission !== "granted") return;
        }
        await sendNotification({ title, body });
    } catch {
        return;
    }
}

function formatBytes(value: number | null): string {
    if (value === null) return "Size unavailable";
    if (value < 1024) return `${value} B`;
    if (value < 1024 * 1024) return `${(value / 1024).toFixed(1)} KB`;
    if (value < 1024 * 1024 * 1024)
        return `${(value / (1024 * 1024)).toFixed(1)} MB`;
    return `${(value / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

function errorMessage(cause: unknown, fallback: string): string {
    if (cause instanceof Error) return cause.message;
    if (typeof cause === "string") return cause;
    return fallback;
}

function providerChoiceFor(kind: ConnectionSummary["kind"]): ProviderChoice {
    switch (kind) {
        case "Sftp":
            return "SFTP";
        case "WebDav":
        case "Nextcloud":
            return "WebDAV";
        case "Ftp":
            return "FTP";
        case "Smb":
            return "SMB";
        case "S3":
            return "S3";
    }
}
