import {
    Cloud,
    HardDrive,
    Settings,
    Activity,
    Plus,
    Pencil,
    FolderOpen,
    Power,
    Trash2,
} from "lucide-react";
import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
} from "@tauri-apps/plugin-notification";
import { FormEvent, useEffect, useState } from "react";
import packageJson from "../package.json";
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
    getConnectionDetails,
    getAvailableDriveLetters,
    installUpdate,
    listConnections,
    listActivity,
    listConflicts,
    openConnectionLocation,
    registerSyncRoot,
    registerDriveMount,
    removeConnection,
    resolveConflict,
    setDriveMountStartup,
    setAutostartEnabled,
    S3ConnectionForm,
    updateConnection,
    unregisterDriveMount,
    unregisterSyncRoot,
} from "./api";

type ProviderChoice = "S3" | "SFTP" | "WebDAV" | "FTP" | "SMB";
type AppView = "connections" | "activity" | "settings" | "add";

type FormDefaults = Record<string, boolean | number | string>;

interface DrivePreference {
    driveLetter: string;
    mountOnStartup: boolean;
}

export function App() {
    const [activeView, setActiveView] = useState<AppView>("connections");
    const [connections, setConnections] = useState<ConnectionSummary[]>([]);
    const [conflicts, setConflicts] = useState<ConflictSummary[]>([]);
    const [activity, setActivity] = useState<ActivitySummary[]>([]);
    const [wizardOpen, setWizardOpen] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [saving, setSaving] = useState(false);
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
    const [mountedDrives, setMountedDrives] = useState<Record<string, string>>(
        {},
    );
    const [drivePreferences, setDrivePreferences] = useState<
        Record<string, DrivePreference>
    >({});
    const [updatingDrive, setUpdatingDrive] = useState<string | null>(null);
    const [availableDriveLetters, setAvailableDriveLetters] = useState<
        string[]
    >([]);
    const [editingConnection, setEditingConnection] =
        useState<ConnectionSummary | null>(null);
    const [formDefaults, setFormDefaults] = useState<FormDefaults>({});
    const [providerChoice, setProviderChoice] = useState<ProviderChoice>("S3");

    useEffect(() => {
        listConnections()
            .then(async (loadedConnections) => {
                setConnections(loadedConnections);
                const details = await Promise.all(
                    loadedConnections.map(async (connection) => {
                        try {
                            return [
                                connection.id,
                                await getConnectionDetails(connection.id),
                            ] as const;
                        } catch {
                            return null;
                        }
                    }),
                );
                const driveConnections = new Set(
                    details.flatMap((entry) =>
                        entry?.[1].configuration.drive_letter ? [entry[0]] : [],
                    ),
                );
                const preferences = Object.fromEntries(
                    details.flatMap((entry) => {
                        const driveLetter =
                            entry?.[1].configuration.drive_letter;
                        return entry && driveLetter
                            ? [
                                  [
                                      entry[0],
                                      {
                                          driveLetter: `${String(driveLetter)}:`,
                                          mountOnStartup:
                                              entry[1].configuration
                                                  .mount_on_startup !== false,
                                      },
                                  ],
                              ]
                            : [];
                    }),
                );
                setDrivePreferences(preferences);
                const registeredRoots = await Promise.all(
                    loadedConnections.map(async (connection) => {
                        if (driveConnections.has(connection.id)) {
                            await unregisterSyncRoot(connection.id).catch(
                                () => undefined,
                            );
                            return null;
                        }
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
                        registeredRoots.flatMap((root) =>
                            root ? [[root[0], root[1].path]] : [],
                        ),
                    ),
                );
                const mounted = await Promise.all(
                    loadedConnections.map(async (connection) => {
                        if (!driveConnections.has(connection.id)) {
                            return null;
                        }
                        if (!preferences[connection.id].mountOnStartup) {
                            return null;
                        }
                        try {
                            const mount = await registerDriveMount(
                                connection.id,
                            );
                            return [connection.id, mount.drive_letter] as const;
                        } catch {
                            return null;
                        }
                    }),
                );
                setMountedDrives(
                    Object.fromEntries(
                        mounted.flatMap((mount) =>
                            mount ? [[mount[0], mount[1]]] : [],
                        ),
                    ),
                );
            })
            .catch(() => {
                setError("Unable to load saved connections.");
            });
    }, []);

    useEffect(() => {
        checkForUpdate()
            .then(setUpdateVersion)
            .catch(() => undefined);
    }, []);

    useEffect(() => {
        if (activeView !== "activity") return;
        listActivity()
            .then(setActivity)
            .catch(() => {
                setError("Unable to load activity history.");
            });
    }, [activeView]);

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

    useEffect(() => {
        if (!wizardOpen) return;
        getAvailableDriveLetters()
            .then(setAvailableDriveLetters)
            .catch(() => setAvailableDriveLetters([]));
    }, [wizardOpen]);

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

    async function handleMountToggle(connection: ConnectionSummary) {
        setUpdatingDrive(connection.id);
        setError(null);
        try {
            if (mountedDrives[connection.id]) {
                await unregisterDriveMount(connection.id);
                setMountedDrives((current) => {
                    const next = { ...current };
                    delete next[connection.id];
                    return next;
                });
            } else {
                const mount = await registerDriveMount(connection.id);
                setMountedDrives((current) => ({
                    ...current,
                    [connection.id]: mount.drive_letter,
                }));
            }
        } catch (cause) {
            setError(errorMessage(cause, "Unable to update the drive mount."));
        } finally {
            setUpdatingDrive(null);
        }
    }

    async function handleDriveStartupChange(
        connection: ConnectionSummary,
        enabled: boolean,
    ) {
        setUpdatingDrive(connection.id);
        setError(null);
        try {
            await setDriveMountStartup(connection.id, enabled);
            setDrivePreferences((current) => ({
                ...current,
                [connection.id]: {
                    ...current[connection.id],
                    mountOnStartup: enabled,
                },
            }));
        } catch (cause) {
            setError(
                errorMessage(cause, "Unable to update the startup setting."),
            );
        } finally {
            setUpdatingDrive(null);
        }
    }

    async function handleOpenLocation(connection: ConnectionSummary) {
        setError(null);
        try {
            await openConnectionLocation(connection.id);
        } catch (cause) {
            setError(errorMessage(cause, "Unable to open Windows Explorer."));
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
                pathStyle: Boolean(configuration.path_style),
                privateKeyPath: String(configuration.private_key_path ?? ""),
                trustOnFirstUse: Boolean(configuration.trust_on_first_use),
                knownHosts: String(configuration.known_hosts ?? ""),
                driveLetter: configuration.drive_letter
                    ? `${String(configuration.drive_letter)}:`
                    : "",
                mountOnStartup: configuration.mount_on_startup !== false,
            });
            setProviderChoice(providerChoiceFor(connection.kind));
            setSftpAuthentication(
                configuration.authentication === "private_key"
                    ? "private_key"
                    : "password",
            );
            setEditingConnection(connection);
            setWizardOpen(true);
            setActiveView("add");
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
            setMountedDrives((current) => {
                const next = { ...current };
                delete next[connection.id];
                return next;
            });
            setDrivePreferences((current) => {
                const next = { ...current };
                delete next[connection.id];
                return next;
            });
        } catch (cause) {
            setError(errorMessage(cause, "Unable to remove connection."));
        }
    }

    async function handleCreate(event: FormEvent<HTMLFormElement>) {
        event.preventDefault();
        const form = event.currentTarget;
        const values = new FormData(form);
        const name = String(values.get("name") ?? "").trim();
        const driveLetter = String(values.get("driveLetter") ?? "").trim();
        const mountOnStartup = values.get("mountOnStartup") === "on";
        const common = {
            name,
            username: String(values.get("username") ?? "").trim(),
            password: String(values.get("password") ?? ""),
            mountOnStartup,
        };
        let connectionOperation: Promise<ConnectionSummary>;
        const endpoint = String(values.get("endpoint") ?? "").trim();
        if (editingConnection) {
            let updateEndpoint = endpoint;
            let configuration: Record<string, unknown>;
            let credentials: Record<string, string>;
            if (providerChoice === "FTP") {
                configuration = {};
                credentials = {
                    username: common.username,
                    password: common.password,
                };
            } else if (providerChoice === "SMB") {
                configuration = {
                    domain: String(values.get("domain") ?? "").trim(),
                };
                credentials = {
                    username: common.username,
                    password: common.password,
                };
            } else if (providerChoice === "WebDAV") {
                configuration = {};
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
            if (driveLetter) {
                configuration.drive_letter = driveLetter;
            }
            configuration.mount_on_startup = mountOnStartup;
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
                driveLetter,
            });
        } else if (providerChoice === "SMB") {
            connectionOperation = createSmbConnection({
                ...common,
                endpoint: String(values.get("endpoint") ?? "").trim(),
                domain: String(values.get("domain") ?? "").trim(),
                driveLetter,
            });
        } else if (providerChoice === "WebDAV") {
            connectionOperation = createWebDavConnection({
                ...common,
                endpoint: String(values.get("endpoint") ?? "").trim(),
                driveLetter,
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
                driveLetter,
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
                driveLetter,
                mountOnStartup,
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
            if (driveLetter) {
                await unregisterSyncRoot(connection.id).catch(() => undefined);
                setExplorerPaths((current) => {
                    const next = { ...current };
                    delete next[connection.id];
                    return next;
                });
            } else {
                try {
                    const root = await registerSyncRoot(connection.id);
                    setExplorerPaths((current) => ({
                        ...current,
                        [connection.id]: root.path,
                    }));
                } catch (cause) {
                    setError(
                        `Connection saved, but its sync folder could not be registered: ${errorMessage(cause, "unknown error")}`,
                    );
                }
            }
            setMountedDrives((current) => {
                const next = { ...current };
                delete next[connection.id];
                return next;
            });
            setDrivePreferences((current) => {
                const next = { ...current };
                if (driveLetter) {
                    next[connection.id] = {
                        driveLetter,
                        mountOnStartup,
                    };
                } else {
                    delete next[connection.id];
                }
                return next;
            });
            try {
                const mount = await registerDriveMount(connection.id);
                setMountedDrives((current) => ({
                    ...current,
                    [connection.id]: mount.drive_letter,
                }));
            } catch (cause) {
                if (driveLetter) {
                    setError(
                        `Connection saved, but ${driveLetter} could not be mounted: ${errorMessage(cause, "unknown error")}`,
                    );
                }
            }
            setWizardOpen(false);
            setActiveView("connections");
            form.reset();
            setEditingConnection(null);
            setFormDefaults({});
            setSftpAuthentication("password");
        } catch (cause) {
            setError(errorMessage(cause, "Unable to save connection."));
        } finally {
            setSaving(false);
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
                    <button
                        className={`nav-item ${activeView === "connections" ? "active" : ""}`}
                        type="button"
                        onClick={() => setActiveView("connections")}
                    >
                        <HardDrive size={17} /> Connections
                    </button>
                    <button
                        className={`nav-item ${activeView === "activity" ? "active" : ""}`}
                        type="button"
                        onClick={() => setActiveView("activity")}
                    >
                        <Activity size={17} /> Activity
                    </button>
                    <button
                        className={`nav-item ${activeView === "settings" ? "active" : ""}`}
                        type="button"
                        onClick={() => setActiveView("settings")}
                    >
                        <Settings size={17} /> Settings
                    </button>
                </nav>
                <div className="sidebar-footer">
                    <span className="status-dot" /> Service ready
                    <small>
                        Build {packageJson.version}
                        {import.meta.env.VITE_BUILD_CHANNEL === "release"
                            ? ""
                            : " DEV"}
                    </small>
                </div>
            </aside>
            {activeView !== "add" && (
                <section className="content">
                    {activeView === "connections" && (
                        <>
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
                                        setWizardOpen(true);
                                        setActiveView("add");
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
                                            <p className="eyebrow">
                                                Needs attention
                                            </p>
                                            <h2 id="conflicts-title">
                                                File conflicts
                                            </h2>
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
                                                    <h3>
                                                        {conflict.remote_path}
                                                    </h3>
                                                    <p>
                                                        Local and remote changes
                                                        differ.
                                                    </p>
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
                                            <p className="eyebrow">
                                                Connected storage
                                            </p>
                                            <h2 id="saved-title">
                                                Your spaces
                                            </h2>
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
                                                    {explorerPaths[
                                                        connection.id
                                                    ] && (
                                                        <p className="explorer-location">
                                                            Sync folder:{" "}
                                                            {
                                                                explorerPaths[
                                                                    connection
                                                                        .id
                                                                ]
                                                            }
                                                        </p>
                                                    )}
                                                    {mountedDrives[
                                                        connection.id
                                                    ] && (
                                                        <p className="explorer-location">
                                                            Drive:{" "}
                                                            {
                                                                mountedDrives[
                                                                    connection
                                                                        .id
                                                                ]
                                                            }
                                                        </p>
                                                    )}
                                                    {drivePreferences[
                                                        connection.id
                                                    ] &&
                                                        !mountedDrives[
                                                            connection.id
                                                        ] && (
                                                            <p className="explorer-location">
                                                                Drive:{" "}
                                                                {
                                                                    drivePreferences[
                                                                        connection
                                                                            .id
                                                                    ]
                                                                        .driveLetter
                                                                }{" "}
                                                                (not mounted)
                                                            </p>
                                                        )}
                                                    {drivePreferences[
                                                        connection.id
                                                    ] && (
                                                        <label className="connection-startup">
                                                            <input
                                                                type="checkbox"
                                                                checked={
                                                                    drivePreferences[
                                                                        connection
                                                                            .id
                                                                    ]
                                                                        .mountOnStartup
                                                                }
                                                                disabled={
                                                                    updatingDrive ===
                                                                    connection.id
                                                                }
                                                                onChange={(
                                                                    event,
                                                                ) =>
                                                                    handleDriveStartupChange(
                                                                        connection,
                                                                        event
                                                                            .target
                                                                            .checked,
                                                                    )
                                                                }
                                                            />
                                                            Mount when Bifrost
                                                            starts
                                                        </label>
                                                    )}
                                                </div>
                                                <span className="connection-state">
                                                    <span
                                                        className={`status-dot ${
                                                            drivePreferences[
                                                                connection.id
                                                            ] &&
                                                            !mountedDrives[
                                                                connection.id
                                                            ]
                                                                ? "disconnected"
                                                                : ""
                                                        }`}
                                                    />{" "}
                                                    {drivePreferences[
                                                        connection.id
                                                    ]
                                                        ? mountedDrives[
                                                              connection.id
                                                          ]
                                                            ? "MOUNTED"
                                                            : "UNMOUNTED"
                                                        : connection.state}
                                                </span>
                                                {drivePreferences[
                                                    connection.id
                                                ] && (
                                                    <button
                                                        className="icon-button"
                                                        type="button"
                                                        aria-label={`${
                                                            mountedDrives[
                                                                connection.id
                                                            ]
                                                                ? "Unmount"
                                                                : "Mount"
                                                        } ${connection.name}`}
                                                        title={
                                                            mountedDrives[
                                                                connection.id
                                                            ]
                                                                ? "Unmount drive"
                                                                : "Mount drive"
                                                        }
                                                        disabled={
                                                            updatingDrive ===
                                                            connection.id
                                                        }
                                                        onClick={() =>
                                                            handleMountToggle(
                                                                connection,
                                                            )
                                                        }
                                                    >
                                                        <Power size={15} />
                                                    </button>
                                                )}
                                                <button
                                                    className="icon-button"
                                                    type="button"
                                                    aria-label={`Open ${connection.name} in Explorer`}
                                                    title="Open in Explorer"
                                                    disabled={
                                                        Boolean(
                                                            drivePreferences[
                                                                connection.id
                                                            ],
                                                        ) &&
                                                        !mountedDrives[
                                                            connection.id
                                                        ]
                                                    }
                                                    onClick={() =>
                                                        handleOpenLocation(
                                                            connection,
                                                        )
                                                    }
                                                >
                                                    <FolderOpen size={15} />
                                                </button>
                                                <button
                                                    className="icon-button"
                                                    type="button"
                                                    aria-label={`Edit ${connection.name}`}
                                                    onClick={() =>
                                                        handleEdit(connection)
                                                    }
                                                >
                                                    <Pencil size={15} />
                                                </button>
                                                <button
                                                    className="icon-button"
                                                    type="button"
                                                    aria-label={`Remove ${connection.name}`}
                                                    onClick={() =>
                                                        handleRemove(connection)
                                                    }
                                                >
                                                    <Trash2 size={15} />
                                                </button>
                                            </article>
                                        ))}
                                    </div>
                                </section>
                            )}
                        </>
                    )}
                    {activeView === "activity" && (
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
                                <span className="muted-label">
                                    Last 100 events
                                </span>
                            </div>
                            {activity.length === 0 ? (
                                <p className="empty-files">
                                    No app activity yet.
                                </p>
                            ) : (
                                <div className="file-list">
                                    {activity.map((event) => (
                                        <div
                                            className="file-row"
                                            key={event.id}
                                        >
                                            <span className="file-icon">
                                                <Activity size={17} />
                                            </span>
                                            <span className="activity-copy">
                                                <strong>
                                                    {activityTitle(event.kind)}
                                                </strong>
                                                {event.remote_path && (
                                                    <span>
                                                        {event.remote_path}
                                                    </span>
                                                )}
                                            </span>
                                            <time
                                                className="file-size"
                                                dateTime={event.created_at}
                                            >
                                                {formatActivityTime(
                                                    event.created_at,
                                                )}
                                            </time>
                                        </div>
                                    ))}
                                </div>
                            )}
                        </section>
                    )}
                    {activeView === "settings" && (
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
                                        handleAutostartChange(
                                            event.target.checked,
                                        )
                                    }
                                />
                                Start Bifrost Drive when I sign in
                            </label>
                        </section>
                    )}
                </section>
            )}
            {activeView === "add" && (
                <section className="content add-view">
                    <section
                        className="wizard connection-form-view"
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
                                onClick={() => {
                                    setWizardOpen(false);
                                    setActiveView("connections");
                                }}
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
                                Windows drive
                                <select
                                    name="driveLetter"
                                    defaultValue={
                                        (formDefaults.driveLetter as string) ??
                                        ""
                                    }
                                >
                                    <option value="">No drive letter</option>
                                    {Array.from(
                                        new Set([
                                            String(
                                                formDefaults.driveLetter ?? "",
                                            ),
                                            ...availableDriveLetters,
                                        ]),
                                    )
                                        .filter(Boolean)
                                        .map((letter) => (
                                            <option key={letter} value={letter}>
                                                {letter}
                                            </option>
                                        ))}
                                </select>
                            </label>
                            <label className="checkbox-row">
                                <input
                                    name="mountOnStartup"
                                    type="checkbox"
                                    defaultChecked={
                                        formDefaults.mountOnStartup !== false
                                    }
                                />
                                Mount this drive when Bifrost starts
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
                                    onClick={() => {
                                        setWizardOpen(false);
                                        setActiveView("connections");
                                    }}
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
                </section>
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

function errorMessage(cause: unknown, fallback: string): string {
    if (cause instanceof Error) return cause.message;
    if (typeof cause === "string") return cause;
    return fallback;
}

function activityTitle(kind: string): string {
    const titles: Record<string, string> = {
        connection_added: "Connection added",
        connection_updated: "Connection updated",
        connection_removed: "Connection removed",
        drive_mounted: "Drive mounted",
        drive_unmounted: "Drive unmounted",
        startup_mount_enabled: "Startup mount enabled",
        startup_mount_disabled: "Startup mount disabled",
        explorer_opened: "Opened in Explorer",
        hydrate: "File downloaded",
        sync: "File synchronized",
        conflict: "Conflict resolved",
    };
    return titles[kind] ?? kind.replaceAll("_", " ");
}

function formatActivityTime(value: string): string {
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
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
