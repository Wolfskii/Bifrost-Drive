import {
    Cloud,
    CloudCog,
    Check,
    Copy,
    Database,
    GitBranch,
    Globe2,
    HardDrive,
    KeyRound,
    LockKeyhole,
    Network,
    Server,
    Settings,
    Activity,
    Plus,
    Pencil,
    FolderOpen,
    Power,
    RefreshCw,
    Trash2,
} from "lucide-react";
import {
    SiAlibabacloud,
    SiBackblaze,
    SiBaidu,
    SiBitbucket,
    SiBox,
    SiCloudflare,
    SiDigitalocean,
    SiDropbox,
    SiFilen,
    SiGithub,
    SiGitlab,
    SiGooglecloudstorage,
    SiGoogledrive,
    SiGooglephotos,
    SiHetzner,
    SiImmich,
    SiMaildotru,
    SiMediafire,
    SiMega,
    SiMinio,
    SiNextcloud,
    SiOpenstack,
    SiProtondrive,
    SiSeafile,
    SiWasabi,
    SiYandexcloud,
    SiZoho,
} from "@icons-pack/react-simple-icons";
import {
    isPermissionGranted,
    requestPermission,
    sendNotification,
} from "@tauri-apps/plugin-notification";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";
import { FormEvent, useEffect, useRef, useState } from "react";
import packageJson from "../package.json";
import { CustomSelect, CustomSelectOption } from "./CustomSelect";
import {
    ActivitySummary,
    authorizeGoogleDrive,
    authorizeGooglePhotos,
    ConnectionSummary,
    ConflictSummary,
    CredentialStoreStatus,
    createFtpConnection,
    createGoogleDriveConnection,
    createGooglePhotosConnection,
    createS3Connection,
    createSftpConnection,
    createSmbConnection,
    createWebDavConnection,
    checkForUpdate,
    getAutostartEnabled,
    getCredentialStoreStatus,
    getStartMinimized,
    getUpdatePopupsEnabled,
    getConnectionDetails,
    getDriveIconPreview,
    getFilesystemIntegration,
    getFilesystemDefaultMountRoot,
    getAvailableDriveLetters,
    getStockDriveIcons,
    installUpdate,
    listConnections,
    listActivity,
    listConflicts,
    openConnectionLocation,
    registerSyncRoot,
    registerDriveMount,
    removeConnection,
    restartApp,
    resolveConflict,
    setDriveMountStartup,
    setAutostartEnabled,
    setStartMinimized,
    setUpdatePopupsEnabled,
    supportsSyncRoots,
    S3ConnectionForm,
    GoogleDriveConnectionForm,
    GoogleDriveAuthorization,
    GooglePhotosConnectionForm,
    FilesystemIntegration,
    StockDriveIcon,
    UpdateInfo,
    updateConnection,
    unregisterDriveMount,
    unregisterSyncRoot,
} from "./api";

type ProviderChoice =
    | "S3"
    | "GoogleDrive"
    | "GooglePhotos"
    | "SFTP"
    | "WebDAV"
    | "FTP"
    | "SMB";
type AppView = "connections" | "activity" | "settings" | "add";
const PRIVACY_POLICY_URL = "https://bifrost.webble.se/privacy/";

type FormDefaults = Record<string, boolean | number | string>;

interface DrivePreference {
    location: string;
    mountOnStartup: boolean;
    iconPreview: string | null;
}

type ReleaseNoteBlock =
    | { kind: "heading"; text: string }
    | { kind: "list"; items: string[] }
    | { kind: "paragraph"; text: string };

export function parseReleaseNotes(body: string): ReleaseNoteBlock[] {
    const blocks: ReleaseNoteBlock[] = [];
    for (const rawLine of body.split(/\r?\n/)) {
        const line = rawLine.trim();
        if (!line) continue;
        const heading = line.match(/^#{1,6}\s+(.+)$/);
        if (heading) {
            blocks.push({ kind: "heading", text: heading[1] });
            continue;
        }
        const listItem = line.match(/^[-*]\s+(.+)$/);
        if (listItem) {
            const previous = blocks.at(-1);
            if (previous?.kind === "list") {
                previous.items.push(listItem[1]);
            } else {
                blocks.push({ kind: "list", items: [listItem[1]] });
            }
            continue;
        }
        blocks.push({ kind: "paragraph", text: line });
    }
    return blocks;
}

function ReleaseNotes({ body }: { body: string }) {
    const blocks = parseReleaseNotes(body);
    if (blocks.length === 0) {
        return (
            <p className="update-notes-empty">
                No release notes were provided.
            </p>
        );
    }
    return (
        <div className="update-notes">
            {blocks.map((block, index) => {
                if (block.kind === "heading") {
                    return <h4 key={`${block.kind}-${index}`}>{block.text}</h4>;
                }
                if (block.kind === "list") {
                    return (
                        <ul key={`${block.kind}-${index}`}>
                            {block.items.map((item) => (
                                <li key={item}>{item}</li>
                            ))}
                        </ul>
                    );
                }
                return <p key={`${block.kind}-${index}`}>{block.text}</p>;
            })}
        </div>
    );
}

export function ConnectionProviderIcon({
    kind,
}: {
    kind: ConnectionSummary["kind"];
}) {
    switch (kind) {
        case "GoogleDrive":
            return <SiGoogledrive size={20} aria-label="Google Drive" />;
        case "GooglePhotos":
            return <SiGooglephotos size={20} aria-label="Google Photos" />;
        case "Nextcloud":
            return <SiNextcloud size={20} aria-label="Nextcloud" />;
        case "S3":
            return <Database size={20} aria-label="S3 object storage" />;
        case "Sftp":
            return <LockKeyhole size={20} aria-label="SFTP" />;
        case "WebDav":
            return <Globe2 size={20} aria-label="WebDAV" />;
        case "Ftp":
            return <Server size={20} aria-label="FTP" />;
        case "Smb":
            return <Network size={20} aria-label="SMB" />;
    }
}

function CommandBox({ label, command }: { label: string; command: string }) {
    const [copied, setCopied] = useState(false);

    async function copyCommand() {
        try {
            await navigator.clipboard.writeText(command);
            setCopied(true);
        } catch {
            setCopied(false);
        }
    }

    return (
        <div className="command-box">
            <div className="command-box-header">
                <span>{label}</span>
                <button
                    className="command-copy-button"
                    type="button"
                    aria-label={`Copy ${label} command`}
                    onClick={() => void copyCommand()}
                >
                    {copied ? <Check size={14} /> : <Copy size={14} />}
                    {copied ? "Copied" : "Copy"}
                </button>
            </div>
            <pre>
                <code>{command}</code>
            </pre>
        </div>
    );
}

function CredentialStoreBanner({
    status,
    restarting,
    onRestart,
}: {
    status: CredentialStoreStatus | null;
    restarting: boolean;
    onRestart: () => void;
}) {
    if (!status || status.available) return null;
    const desktop = status.desktop_environment?.toLowerCase() ?? "";
    const isKde = desktop.includes("kde") || desktop.includes("plasma");
    const distribution = status.linux_distribution;
    const installCommand = distribution
        ? ["fedora", "rhel", "centos", "rocky", "almalinux"].includes(
              distribution,
          )
            ? {
                  label: "Fedora / RHEL",
                  command: "sudo dnf install gnome-keyring libsecret",
              }
            : ["ubuntu", "debian", "linuxmint", "pop"].includes(distribution)
              ? {
                    label: "Ubuntu / Debian",
                    command: "sudo apt install gnome-keyring libsecret-1-0",
                }
              : ["arch", "manjaro", "endeavouros"].includes(distribution)
                ? {
                      label: "Arch",
                      command: "sudo pacman -S gnome-keyring libsecret",
                  }
                : null
        : null;

    return (
        <section className="system-check" role="alert">
            <div>
                <strong>{status.provider} needs attention</strong>
                <p>{status.message}</p>
                {status.platform === "linux" && (
                    <>
                        {isKde ? (
                            <>
                                <p>
                                    KDE Wallet is installed; no package command
                                    is needed. Enable its Secret Service
                                    interface:
                                </p>
                                <ol className="system-steps">
                                    <li>
                                        Open System Settings, then KDE Wallet.
                                    </li>
                                    <li>
                                        Check{" "}
                                        <strong>
                                            Use KWallet for the Secret Service
                                            interface
                                        </strong>{" "}
                                        and apply the change.
                                    </li>
                                    <li>
                                        Unlock the default wallet, then restart
                                        Bifrost. If it still fails, sign out and
                                        back in once.
                                    </li>
                                </ol>
                            </>
                        ) : (
                            <>
                                <p>
                                    Install and start a Secret Service provider,
                                    unlock its default wallet, then restart
                                    Bifrost.
                                </p>
                                {installCommand && (
                                    <div className="system-commands">
                                        <CommandBox {...installCommand} />
                                    </div>
                                )}
                            </>
                        )}
                    </>
                )}
                {status.platform === "windows" && (
                    <p>
                        Check that your Windows profile can access Credential
                        Manager, then restart Bifrost.
                    </p>
                )}
                {status.platform === "macos" && (
                    <p>
                        Unlock your login keychain and allow Bifrost to access
                        it, then restart Bifrost.
                    </p>
                )}
            </div>
            <button
                className="secondary-button"
                type="button"
                onClick={onRestart}
                disabled={restarting}
            >
                <RefreshCw size={15} />
                {restarting ? "Restarting..." : "Restart Bifrost"}
            </button>
        </section>
    );
}

const planned = { disabled: true, badge: "Planned" } as const;
const providerOptions: CustomSelectOption<string>[] = [
    {
        value: "SFTP",
        label: "SFTP server",
        group: "Protocols",
        description: "SSH File Transfer Protocol",
        icon: <LockKeyhole size={19} />,
    },
    {
        value: "WebDAV",
        label: "WebDAV server",
        group: "Protocols",
        icon: <Globe2 size={19} />,
    },
    {
        value: "FTP",
        label: "FTP / FTPS server",
        group: "Protocols",
        icon: <Server size={19} />,
    },
    {
        value: "SMB",
        label: "Samba / SMB share",
        group: "Protocols",
        icon: <Network size={19} />,
    },
    {
        value: "nfs",
        label: "NFS",
        group: "Protocols",
        icon: <Network size={19} />,
        ...planned,
    },
    ...[
        "Custom S3-compatible",
        "Amazon S3",
        "Cloudflare R2",
        "Backblaze B2",
        "Wasabi",
        "DigitalOcean Spaces",
        "MinIO",
        "Storj",
        "IDrive e2",
        "Vultr Object Storage",
        "Scaleway Object Storage",
        "IONOS Object Storage",
        "Hetzner Object Storage",
        "Tigris",
        "Linode/Akamai Object Storage",
        "Oracle Cloud Object Storage",
        "IBM Cloud Object Storage",
        "Alibaba Cloud OSS",
        "Tencent Cloud COS",
        "OVHcloud Object Storage",
        "Exoscale Object Storage",
        "Impossible Cloud",
        "Cloudian",
        "Ceph RGW",
    ].map((label, index) => ({
        value: `s3-${index}`,
        label,
        group: "S3-compatible object storage",
        description: "Uses the implemented S3 connection flow",
        icon:
            label === "Cloudflare R2" ? (
                <SiCloudflare size={19} />
            ) : label === "Backblaze B2" ? (
                <SiBackblaze size={19} />
            ) : label === "Wasabi" ? (
                <SiWasabi size={19} />
            ) : label === "DigitalOcean Spaces" ? (
                <SiDigitalocean size={19} />
            ) : label === "MinIO" ? (
                <SiMinio size={19} />
            ) : label === "Hetzner Object Storage" ? (
                <SiHetzner size={19} />
            ) : label === "Alibaba Cloud OSS" ? (
                <SiAlibabacloud size={19} />
            ) : label === "Ceph RGW" ? (
                <SiOpenstack size={19} />
            ) : (
                <Database size={19} />
            ),
    })),
    {
        value: "google-drive",
        label: "Google Drive",
        group: "Cloud services",
        icon: <SiGoogledrive size={19} />,
    },
    {
        value: "google-photos",
        label: "Google Photos",
        group: "Cloud services",
        description: "Official access to media and albums created by Bifrost",
        icon: <SiGooglephotos size={19} />,
    },
    ...[
        ["dropbox", "Dropbox", <SiDropbox size={19} />],
        ["mega", "MEGA", <SiMega size={19} />],
        ["mailru", "Mail.ru Cloud", <SiMaildotru size={19} />],
        ["yandex", "Yandex Disk", <SiYandexcloud size={19} />],
        ["pcloud", "pCloud", <Cloud size={19} />],
        ["onedrive", "OneDrive", <Cloud size={19} />],
        ["box", "Box", <SiBox size={19} />],
        ["zoho", "Zoho WorkDrive", <SiZoho size={19} />],
        ["azure-blob", "Azure Blob Storage", <CloudCog size={19} />],
        ["gcs", "Google Cloud Storage", <SiGooglecloudstorage size={19} />],
        ["proton", "Proton Drive", <SiProtondrive size={19} />],
        ["koofr", "Koofr", <Cloud size={19} />],
        ["filen", "Filen", <SiFilen size={19} />],
        ["hetzner-storage", "Hetzner Storage", <SiHetzner size={19} />],
        ["nextcloud", "Nextcloud", <SiNextcloud size={19} />],
        ["4shared", "4shared", <Cloud size={19} />],
        ["mediafire", "MediaFire", <SiMediafire size={19} />],
        ["jottacloud", "Jottacloud", <Cloud size={19} />],
        ["immich", "Immich", <SiImmich size={19} />],
        ["opendrive", "OpenDrive", <Cloud size={19} />],
        ["nordlocker", "NordLocker", <KeyRound size={19} />],
        ["sharepoint", "SharePoint Online", <CloudCog size={19} />],
        ["seafile", "Seafile", <SiSeafile size={19} />],
        ["baidu", "Baidu Netdisk", <SiBaidu size={19} />],
        ["alibaba-drive", "Alibaba Cloud Drive", <SiAlibabacloud size={19} />],
        ["tencent", "Tencent Weiyun", <Cloud size={19} />],
    ].map(([value, label, icon]) => ({
        value: value as string,
        label: label as string,
        group: "Cloud services",
        icon,
        ...planned,
    })),
    ...[
        ["github", "GitHub", <SiGithub size={19} />],
        ["gitlab", "GitLab", <SiGitlab size={19} />],
        ["bitbucket", "Bitbucket", <SiBitbucket size={19} />],
        ["azure-devops", "Azure DevOps", <GitBranch size={19} />],
    ].map(([value, label, icon]) => ({
        value: value as string,
        label: label as string,
        group: "Git",
        icon,
        ...planned,
    })),
];

function providerChoiceFromSelection(value: string): ProviderChoice {
    if (value.startsWith("s3-")) return "S3";
    if (value === "google-drive") return "GoogleDrive";
    if (value === "google-photos") return "GooglePhotos";
    if (["SFTP", "WebDAV", "FTP", "SMB"].includes(value)) {
        return value as ProviderChoice;
    }
    return "S3";
}

export function App() {
    const [activeView, setActiveView] = useState<AppView>("connections");
    const [connections, setConnections] = useState<ConnectionSummary[]>([]);
    const [conflicts, setConflicts] = useState<ConflictSummary[]>([]);
    const [activity, setActivity] = useState<ActivitySummary[]>([]);
    const [wizardOpen, setWizardOpen] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [credentialStoreStatus, setCredentialStoreStatus] =
        useState<CredentialStoreStatus | null>(null);
    const [restartingApp, setRestartingApp] = useState(false);
    const [saving, setSaving] = useState(false);
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
    const [installingUpdate, setInstallingUpdate] = useState(false);
    const [checkingForUpdate, setCheckingForUpdate] = useState(false);
    const [showUpdatePopups, setShowUpdatePopups] = useState(true);
    const [updatingUpdatePopups, setUpdatingUpdatePopups] = useState(false);
    const [updatePopupOpen, setUpdatePopupOpen] = useState(false);
    const [autostartEnabled, setAutostartEnabledState] = useState(false);
    const [updatingAutostart, setUpdatingAutostart] = useState(false);
    const [startMinimized, setStartMinimizedState] = useState(false);
    const [updatingStartMinimized, setUpdatingStartMinimized] = useState(false);
    const [sftpAuthentication, setSftpAuthentication] = useState<
        "password" | "private_key"
    >("password");
    const [driveType, setDriveType] = useState<"network" | "local">("network");
    const [driveIcon, setDriveIcon] = useState("system");
    const [customDriveIcon, setCustomDriveIcon] = useState("");
    const [customDriveIconPreview, setCustomDriveIconPreview] = useState("");
    const [stockDriveIcons, setStockDriveIcons] = useState<StockDriveIcon[]>(
        [],
    );
    const [iconPickerOpen, setIconPickerOpen] = useState(false);
    const [iconPickerTab, setIconPickerTab] = useState<
        "windows" | "bifrost" | "custom"
    >("windows");
    const iconPickerRef = useRef<HTMLDivElement>(null);
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
    const [providerSelection, setProviderSelection] = useState("s3-1");
    const [googleAuthorization, setGoogleAuthorization] =
        useState<GoogleDriveAuthorization | null>(null);
    const [googleSigningIn, setGoogleSigningIn] = useState(false);
    const [driveLetter, setDriveLetter] = useState("");
    const [defaultMountRoot, setDefaultMountRoot] = useState("");
    const [mountRoot, setMountRoot] = useState("");
    const [syncRootsSupported, setSyncRootsSupported] = useState(false);
    const [filesystemIntegration, setFilesystemIntegration] =
        useState<FilesystemIntegration>("none");

    async function checkCredentialStore() {
        try {
            setCredentialStoreStatus(await getCredentialStoreStatus());
        } catch {
            setCredentialStoreStatus(null);
        }
    }

    async function handleRestartApp() {
        setRestartingApp(true);
        try {
            await restartApp();
        } catch (cause) {
            setRestartingApp(false);
            setError(errorMessage(cause, "Unable to restart Bifrost."));
        }
    }

    async function handleGoogleSignIn() {
        setGoogleSigningIn(true);
        setError(null);
        try {
            setGoogleAuthorization(
                await (providerChoice === "GooglePhotos"
                    ? authorizeGooglePhotos()
                    : authorizeGoogleDrive()),
            );
        } catch (cause) {
            setError(errorMessage(cause, "Google sign-in failed."));
        } finally {
            setGoogleSigningIn(false);
        }
    }

    function closeWizard() {
        setWizardOpen(false);
        setActiveView("connections");
        setError(null);
    }

    useEffect(() => {
        void checkCredentialStore();
    }, []);

    useEffect(() => {
        Promise.all([
            listConnections(),
            supportsSyncRoots(),
            getFilesystemIntegration(),
            getFilesystemDefaultMountRoot(),
        ])
            .then(
                async ([
                    loadedConnections,
                    supportsRoots,
                    filesystemKind,
                    defaultRoot,
                ]) => {
                    setSyncRootsSupported(supportsRoots);
                    setFilesystemIntegration(filesystemKind);
                    setDefaultMountRoot(defaultRoot);
                    setMountRoot(defaultRoot);
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
                            filesystemKind === "linux" ||
                            filesystemKind === "macos" ||
                            entry?.[1].configuration.drive_letter
                                ? [entry?.[0]].filter((id): id is string =>
                                      Boolean(id),
                                  )
                                : [],
                        ),
                    );
                    const preferences = Object.fromEntries(
                        details.flatMap((entry) => {
                            const driveLetter =
                                entry?.[1].configuration.drive_letter;
                            return entry &&
                                (driveLetter ||
                                    filesystemKind === "linux" ||
                                    filesystemKind === "macos")
                                ? [
                                      [
                                          entry[0],
                                          {
                                              location: driveLetter
                                                  ? `${String(driveLetter)}:`
                                                  : filesystemKind === "macos"
                                                    ? `Finder > Locations > ${entry[1].summary.name}`
                                                    : `${String(entry[1].configuration.mount_root ?? defaultRoot).replace(/\/$/, "")}/${entry[1].summary.name}`,
                                              mountOnStartup:
                                                  entry[1].configuration
                                                      .mount_on_startup !==
                                                  false,
                                              iconPreview:
                                                  entry[1].drive_icon_preview,
                                          },
                                      ],
                                  ]
                                : [];
                        }),
                    );
                    setDrivePreferences(preferences);
                    const registeredRoots = supportsRoots
                        ? await Promise.all(
                              loadedConnections.map(async (connection) => {
                                  if (driveConnections.has(connection.id)) {
                                      await unregisterSyncRoot(
                                          connection.id,
                                      ).catch(() => undefined);
                                      return null;
                                  }
                                  try {
                                      const root = await registerSyncRoot(
                                          connection.id,
                                      );
                                      return [connection.id, root] as const;
                                  } catch {
                                      return null;
                                  }
                              }),
                          )
                        : [];
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
                                return [connection.id, mount.location] as const;
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
                },
            )
            .catch(() => {
                setError("Unable to load saved connections.");
            });
    }, []);

    useEffect(() => {
        if (!iconPickerOpen) return;
        const closePicker = (event: PointerEvent) => {
            if (
                iconPickerRef.current &&
                !iconPickerRef.current.contains(event.target as Node)
            ) {
                setIconPickerOpen(false);
            }
        };
        document.addEventListener("pointerdown", closePicker);
        return () => document.removeEventListener("pointerdown", closePicker);
    }, [iconPickerOpen]);
    async function checkForUpdates(showPopup = false) {
        setCheckingForUpdate(true);
        setError(null);
        try {
            const update = await checkForUpdate();
            setUpdateInfo(update);
            if (showPopup && update && showUpdatePopups) {
                setUpdatePopupOpen(true);
            }
            return update;
        } catch (cause) {
            setError(errorMessage(cause, "Unable to check for updates."));
            return null;
        } finally {
            setCheckingForUpdate(false);
        }
    }

    useEffect(() => {
        Promise.all([checkForUpdate(), getUpdatePopupsEnabled()])
            .then(([update, popupsEnabled]) => {
                setUpdateInfo(update);
                setShowUpdatePopups(popupsEnabled);
                if (update && popupsEnabled) {
                    setUpdatePopupOpen(true);
                }
            })
            .catch(() => undefined);
    }, []);

    useEffect(() => {
        if (activeView !== "activity") return;
        listActivity()
            .then(setActivity)
            .catch(() => setError("Unable to load activity history."));
    }, [activeView]);

    useEffect(() => {
        listConflicts()
            .then(setConflicts)
            .catch(() => setError("Unable to load unresolved conflicts."));
    }, []);

    useEffect(() => {
        getAutostartEnabled()
            .then(setAutostartEnabledState)
            .catch(() => undefined);
        getStartMinimized()
            .then(setStartMinimizedState)
            .catch(() => undefined);
    }, []);

    useEffect(() => {
        if (!wizardOpen) return;
        getAvailableDriveLetters()
            .then(setAvailableDriveLetters)
            .catch(() => setAvailableDriveLetters([]));
        getStockDriveIcons()
            .then(setStockDriveIcons)
            .catch(() => setStockDriveIcons([]));
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

    async function handleStartMinimizedChange(enabled: boolean) {
        setUpdatingStartMinimized(true);
        setError(null);
        try {
            await setStartMinimized(enabled);
            setStartMinimizedState(enabled);
        } catch (cause) {
            setError(
                cause instanceof Error
                    ? cause.message
                    : "Unable to update minimized startup settings.",
            );
        } finally {
            setUpdatingStartMinimized(false);
        }
    }

    async function handleUpdatePopupsChange(enabled: boolean) {
        setUpdatingUpdatePopups(true);
        setError(null);
        try {
            await setUpdatePopupsEnabled(enabled);
            setShowUpdatePopups(enabled);
        } catch (cause) {
            setError(errorMessage(cause, "Unable to update update settings."));
        } finally {
            setUpdatingUpdatePopups(false);
        }
    }

    async function handleInstallUpdate() {
        setInstallingUpdate(true);
        setError(null);
        try {
            await installUpdate();
        } catch (cause) {
            setError(errorMessage(cause, "Unable to install the update."));
            setInstallingUpdate(false);
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
                    [connection.id]: mount.location,
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
            setError(errorMessage(cause, "Unable to open this location."));
        }
    }

    async function handleBrowseDriveIcon() {
        const selected = await openFileDialog({
            multiple: false,
            directory: false,
            filters: [
                {
                    name: "Drive images and icons",
                    extensions: [
                        "ico",
                        "exe",
                        "dll",
                        "png",
                        "jpg",
                        "jpeg",
                        "webp",
                    ],
                },
            ],
        });
        if (typeof selected === "string") {
            setCustomDriveIcon(selected);
            setDriveIcon("custom");
            setCustomDriveIconPreview(
                await getDriveIconPreview(selected).catch(() => ""),
            );
        }
    }

    async function handleBrowseMountRoot() {
        const selected = await openFileDialog({
            multiple: false,
            directory: true,
            defaultPath: mountRoot || defaultMountRoot || undefined,
        });
        if (typeof selected === "string") {
            setMountRoot(selected);
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
                accessToken: "",
                pathStyle: Boolean(configuration.path_style),
                privateKeyPath: String(configuration.private_key_path ?? ""),
                trustOnFirstUse: Boolean(configuration.trust_on_first_use),
                knownHosts: String(configuration.known_hosts ?? ""),
                driveLetter: configuration.drive_letter
                    ? `${String(configuration.drive_letter)}:`
                    : "",
                mountOnStartup: configuration.mount_on_startup !== false,
                mountRoot: String(configuration.mount_root ?? defaultMountRoot),
                openWorkspaceInNativeApps:
                    configuration.workspace_open_mode !== "browser",
                legacyFolderId: String(configuration.legacy_folder_id ?? ""),
                legacyFolderPath: "",
            });
            const configuredIcon = String(configuration.drive_icon ?? "system");
            const builtInIcon =
                [
                    "system",
                    "bifrost",
                    "windows_local",
                    "windows_network",
                ].includes(configuredIcon) ||
                configuredIcon.startsWith("stock:") ||
                configuredIcon.startsWith("shell32:");
            setDriveType(
                configuration.drive_type === "local" ? "local" : "network",
            );
            setDriveIcon(builtInIcon ? configuredIcon : "custom");
            setCustomDriveIcon(builtInIcon ? "" : configuredIcon);
            setCustomDriveIconPreview(
                builtInIcon ? "" : (details.drive_icon_preview ?? ""),
            );
            setProviderChoice(providerChoiceFor(connection.kind));
            setGoogleAuthorization(null);
            setProviderSelection(
                connection.kind === "S3"
                    ? "s3-0"
                    : providerChoiceFor(connection.kind),
            );
            setDriveLetter(
                configuration.drive_letter
                    ? `${String(configuration.drive_letter)}:`
                    : "",
            );
            setMountRoot(String(configuration.mount_root ?? defaultMountRoot));
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
        const selectedMountRoot = String(
            values.get("mountRoot") ?? mountRoot,
        ).trim();
        const mountOnStartup = values.get("mountOnStartup") === "on";
        const workspaceOpenMode =
            values.get("openWorkspaceInNativeApps") === "on"
                ? "native_apps"
                : "browser";
        const selectedDriveType = String(
            values.get("driveType") ?? "network",
        ) as "network" | "local";
        const selectedDriveIcon =
            String(values.get("driveIcon") ?? "system") === "custom"
                ? customDriveIcon
                : String(values.get("driveIcon") ?? "system");
        const common = {
            name,
            username: String(values.get("username") ?? "").trim(),
            password: String(values.get("password") ?? ""),
            mountOnStartup,
            mountRoot: selectedMountRoot,
            driveType: selectedDriveType,
            driveIcon: selectedDriveIcon,
        };
        if (
            (providerChoice === "GoogleDrive" ||
                providerChoice === "GooglePhotos") &&
            !googleAuthorization &&
            !editingConnection
        ) {
            setError("Sign in with Google before mounting this connection.");
            return;
        }
        let connectionOperation: Promise<ConnectionSummary>;
        const endpoint = String(values.get("endpoint") ?? "").trim();
        if (editingConnection) {
            let updateEndpoint = endpoint;
            let configuration: Record<string, unknown>;
            let credentials: Record<string, unknown>;
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
            } else if (providerChoice === "GoogleDrive") {
                updateEndpoint = "https://www.googleapis.com/drive/v3";
                configuration = {
                    workspace_open_mode: workspaceOpenMode,
                };
                credentials = googleAuthorization
                    ? {
                          access_token: googleAuthorization.access_token,
                          refresh_token: googleAuthorization.refresh_token,
                          expires_at: googleAuthorization.expires_at,
                      }
                    : {};
            } else if (providerChoice === "GooglePhotos") {
                updateEndpoint = "https://photoslibrary.googleapis.com/v1";
                configuration = {};
                credentials = googleAuthorization
                    ? {
                          access_token: googleAuthorization.access_token,
                          refresh_token: googleAuthorization.refresh_token,
                          expires_at: googleAuthorization.expires_at,
                      }
                    : {};
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
            if (filesystemIntegration === "linux") {
                configuration.mount_root = selectedMountRoot;
            }
            configuration.drive_type = selectedDriveType;
            configuration.drive_icon = selectedDriveIcon;
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
        } else if (providerChoice === "GoogleDrive") {
            const form: GoogleDriveConnectionForm = {
                name,
                accessToken:
                    googleAuthorization?.access_token ??
                    String(values.get("accessToken") ?? ""),
                refreshToken: googleAuthorization?.refresh_token ?? null,
                expiresAt: googleAuthorization?.expires_at ?? null,
                legacyFolderId: String(values.get("legacyFolderId") ?? "").trim(),
                legacyFolderPath: String(values.get("legacyFolderPath") ?? "").trim(),
                driveLetter,
                mountOnStartup,
                mountRoot: selectedMountRoot,
                driveType: selectedDriveType,
                driveIcon: selectedDriveIcon,
                workspaceOpenMode,
            };
            connectionOperation = createGoogleDriveConnection(form);
        } else if (providerChoice === "GooglePhotos") {
            const form: GooglePhotosConnectionForm = {
                name,
                accessToken:
                    googleAuthorization?.access_token ??
                    String(values.get("accessToken") ?? ""),
                refreshToken: googleAuthorization?.refresh_token ?? null,
                expiresAt: googleAuthorization?.expires_at ?? null,
                driveLetter,
                mountOnStartup,
                mountRoot: selectedMountRoot,
                driveType: selectedDriveType,
                driveIcon: selectedDriveIcon,
            };
            connectionOperation = createGooglePhotosConnection(form);
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
                mountRoot: selectedMountRoot,
                driveType: selectedDriveType,
                driveIcon: selectedDriveIcon,
            };
            connectionOperation = createS3Connection(form);
        }
        setSaving(true);
        setError(null);
        try {
            const connection = await connectionOperation;
            if (editingConnection && mountedDrives[connection.id]) {
                await unregisterDriveMount(connection.id);
            }
            setConnections((current) => [
                ...current.filter((item) => item.id !== editingConnection?.id),
                connection,
            ]);
            if (driveLetter && syncRootsSupported) {
                await unregisterSyncRoot(connection.id).catch(() => undefined);
                setExplorerPaths((current) => {
                    const next = { ...current };
                    delete next[connection.id];
                    return next;
                });
            } else if (syncRootsSupported) {
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
                if (
                    driveLetter ||
                    filesystemIntegration === "linux" ||
                    filesystemIntegration === "macos"
                ) {
                    next[connection.id] = {
                        location:
                            filesystemIntegration === "linux"
                                ? `${selectedMountRoot.replace(/\/$/, "")}/${name}`
                                : filesystemIntegration === "macos"
                                  ? `Finder > Locations > ${name}`
                                  : driveLetter,
                        mountOnStartup,
                        iconPreview:
                            customDriveIconPreview ||
                            stockDriveIcons.find(
                                (icon) => icon.value === selectedDriveIcon,
                            )?.preview ||
                            null,
                    };
                } else {
                    delete next[connection.id];
                }
                return next;
            });
            try {
                if (filesystemIntegration === "none") {
                    throw new Error(
                        "Native filesystem integration is unavailable",
                    );
                }
                const mount = await registerDriveMount(connection.id);
                setMountedDrives((current) => ({
                    ...current,
                    [connection.id]: mount.location,
                }));
            } catch (cause) {
                if (
                    driveLetter ||
                    filesystemIntegration === "linux" ||
                    filesystemIntegration === "macos"
                ) {
                    setError(
                        `Connection saved, but its filesystem location could not be mounted: ${errorMessage(cause, "unknown error")}`,
                    );
                }
            }
            setWizardOpen(false);
            setActiveView("connections");
            form.reset();
            setEditingConnection(null);
            setFormDefaults({});
            setGoogleAuthorization(null);
            setSftpAuthentication("password");
            setDriveType("network");
            setDriveIcon("system");
            setCustomDriveIcon("");
            setCustomDriveIconPreview("");
            setProviderSelection("s3-1");
            setDriveLetter("");
            setMountRoot(defaultMountRoot);
        } catch (cause) {
            const message = errorMessage(cause, "Unable to save connection.");
            setError(message);
            if (/native credential store/i.test(message)) {
                setCredentialStoreStatus((current) => ({
                    available: false,
                    platform: current?.platform ?? "unknown",
                    provider: current?.provider ?? "Native credential store",
                    message,
                    desktop_environment: current?.desktop_environment ?? null,
                    linux_distribution: current?.linux_distribution ?? null,
                }));
            }
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

    const selectedStockIcon = stockDriveIcons.find(
        (icon) =>
            icon.value === driveIcon ||
            (driveIcon === "windows_local" && icon.value === "stock:8") ||
            (driveIcon === "windows_network" && icon.value === "stock:9"),
    );
    const selectedDriveIconLabel =
        driveIcon === "system"
            ? "System default"
            : driveIcon === "bifrost"
              ? "Bifrost"
              : driveIcon === "custom"
                ? "Custom icon"
                : (selectedStockIcon?.label ?? "Windows icon");
    const selectedProvider = providerOptions.find(
        (option) => option.value === providerSelection,
    );
    const driveLetterOptions: CustomSelectOption<string>[] = [
        {
            value: "",
            label: "No drive letter",
            icon: <HardDrive size={18} />,
        },
        ...Array.from(
            new Set([driveLetter, ...availableDriveLetters].filter(Boolean)),
        ).map((letter) => ({
            value: letter,
            label: letter,
            icon: <HardDrive size={18} />,
        })),
    ];

    return (
        <main className="app-shell">
            <aside className="sidebar">
                <div className="brand-lockup">
                    <div className="brand-mark" aria-hidden="true">
                        <img src="/bifrost-dark.svg" alt="" />
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
                        onClick={closeWizard}
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
                    <CredentialStoreBanner
                        status={credentialStoreStatus}
                        restarting={restartingApp}
                        onRestart={() => void handleRestartApp()}
                    />
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
                                        setProviderChoice("S3");
                                        setGoogleAuthorization(null);
                                        setSftpAuthentication("password");
                                        setDriveType("network");
                                        setDriveIcon("system");
                                        setCustomDriveIcon("");
                                        setCustomDriveIconPreview("");
                                        setProviderSelection("s3-1");
                                        setDriveLetter("");
                                        setMountRoot(defaultMountRoot);
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
                                                    {drivePreferences[
                                                        connection.id
                                                    ]?.iconPreview ? (
                                                        <img
                                                            src={
                                                                drivePreferences[
                                                                    connection
                                                                        .id
                                                                ].iconPreview ??
                                                                ""
                                                            }
                                                            alt=""
                                                        />
                                                    ) : (
                                                        <ConnectionProviderIcon
                                                            kind={
                                                                connection.kind
                                                            }
                                                        />
                                                    )}
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
                                                            {filesystemIntegration ===
                                                            "windows"
                                                                ? "Drive: "
                                                                : filesystemIntegration ===
                                                                    "macos"
                                                                  ? "Finder: "
                                                                  : "Folder: "}
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
                                                                {filesystemIntegration ===
                                                                "windows"
                                                                    ? "Drive: "
                                                                    : filesystemIntegration ===
                                                                        "macos"
                                                                      ? "Finder: "
                                                                      : "Folder: "}
                                                                {
                                                                    drivePreferences[
                                                                        connection
                                                                            .id
                                                                    ].location
                                                                }{" "}
                                                                (not mounted)
                                                            </p>
                                                        )}
                                                    {drivePreferences[
                                                        connection.id
                                                    ] && (
                                                        <div className="connection-startup">
                                                            <input
                                                                type="checkbox"
                                                                aria-label="Mount when Bifrost starts"
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
                                                        </div>
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
                                                                : filesystemIntegration ===
                                                                    "windows"
                                                                  ? "Mount drive"
                                                                  : filesystemIntegration ===
                                                                      "macos"
                                                                    ? "Add to Finder"
                                                                    : "Mount folder"
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
                                                    aria-label={`Open ${connection.name} location`}
                                                    title="Open location"
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
                            <div className="checkbox-row">
                                <input
                                    type="checkbox"
                                    aria-label="Start Bifrost Drive when I sign in"
                                    checked={autostartEnabled}
                                    disabled={updatingAutostart}
                                    onChange={(event) =>
                                        handleAutostartChange(
                                            event.target.checked,
                                        )
                                    }
                                />
                                Start Bifrost Drive when I sign in
                            </div>
                            <div className="checkbox-row settings-option">
                                <input
                                    type="checkbox"
                                    aria-label="Start minimized in the notification tray"
                                    checked={startMinimized}
                                    disabled={updatingStartMinimized}
                                    onChange={(event) =>
                                        handleStartMinimizedChange(
                                            event.target.checked,
                                        )
                                    }
                                />
                                <span>
                                    Start minimized in the notification tray
                                    <small>
                                        Keep mounted drives available without
                                        opening the main window.
                                    </small>
                                </span>
                            </div>
                            <div className="settings-update-section">
                                <div className="section-heading">
                                    <div>
                                        <p className="eyebrow">Software</p>
                                        <h3>Updates</h3>
                                    </div>
                                    <button
                                        className="secondary-button"
                                        type="button"
                                        disabled={checkingForUpdate}
                                        onClick={() => void checkForUpdates()}
                                    >
                                        {checkingForUpdate
                                            ? "Checking..."
                                            : "Check for updates"}
                                    </button>
                                </div>
                                {updateInfo ? (
                                    <div
                                        className="update-details"
                                        role="status"
                                    >
                                        <strong>
                                            Version {updateInfo.version} is
                                            available.
                                        </strong>
                                        <ReleaseNotes body={updateInfo.body} />
                                        <button
                                            className="primary-button"
                                            type="button"
                                            disabled={installingUpdate}
                                            onClick={() =>
                                                void handleInstallUpdate()
                                            }
                                        >
                                            {installingUpdate
                                                ? "Installing..."
                                                : "Update now"}
                                        </button>
                                    </div>
                                ) : (
                                    <p
                                        className="settings-status"
                                        role="status"
                                    >
                                        No update is currently available.
                                    </p>
                                )}
                                <label className="checkbox-row settings-option">
                                    <input
                                        type="checkbox"
                                        aria-label="Show update popups"
                                        checked={showUpdatePopups}
                                        disabled={updatingUpdatePopups}
                                        onChange={(event) =>
                                            void handleUpdatePopupsChange(
                                                event.target.checked,
                                            )
                                        }
                                    />
                                    Show update popups
                                </label>
                            </div>
                            <p className="legal-notice">
                                Bifrost stores credentials in your operating
                                system’s secure credential service and uses
                                Google Drive data only to provide requested
                                storage features. Read the{" "}
                                <a
                                    href={PRIVACY_POLICY_URL}
                                    target="_blank"
                                    rel="noreferrer"
                                >
                                    Privacy Policy
                                </a>
                                .
                            </p>
                        </section>
                    )}
                </section>
            )}
            {activeView === "add" && (
                <section className="content add-view">
                    <CredentialStoreBanner
                        status={credentialStoreStatus}
                        restarting={restartingApp}
                        onRestart={() => void handleRestartApp()}
                    />
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
                                    {selectedProvider?.label ?? providerChoice}
                                </h2>
                            </div>
                            <button
                                className="icon-button"
                                type="button"
                                aria-label="Close"
                                onClick={closeWizard}
                            >
                                ×
                            </button>
                        </div>
                        <p className="wizard-copy">
                            Credentials are stored in the native credential
                            service and never in the app database.
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
                            <CustomSelect
                                className="provider-select"
                                label="Storage type"
                                value={providerSelection}
                                options={providerOptions}
                                onChange={(value) => {
                                    setProviderSelection(value);
                                    setProviderChoice(
                                        providerChoiceFromSelection(value),
                                    );
                                }}
                            />
                            <label>
                                Connection name
                                <input
                                    name="name"
                                    required
                                    defaultValue={formDefaults.name as string}
                                    placeholder="Production S3"
                                />
                            </label>
                            <div
                                className="windows-mount-options"
                                hidden={filesystemIntegration !== "windows"}
                            >
                                <CustomSelect
                                    label="Windows drive"
                                    name="driveLetter"
                                    value={driveLetter}
                                    options={driveLetterOptions}
                                    onChange={setDriveLetter}
                                />
                                <div className="form-grid">
                                    <CustomSelect
                                        label="Drive type"
                                        name="driveType"
                                        value={driveType}
                                        options={[
                                            {
                                                value: "network",
                                                label: "Network location",
                                                icon: <Network size={18} />,
                                            },
                                            {
                                                value: "local",
                                                label: "Local drive",
                                                icon: <HardDrive size={18} />,
                                            },
                                        ]}
                                        onChange={setDriveType}
                                    />
                                    <div
                                        className="drive-icon-field"
                                        ref={iconPickerRef}
                                    >
                                        <span>Drive icon</span>
                                        <input
                                            name="driveIcon"
                                            type="hidden"
                                            value={driveIcon}
                                        />
                                        <button
                                            className="icon-picker-trigger"
                                            type="button"
                                            aria-haspopup="dialog"
                                            aria-expanded={iconPickerOpen}
                                            onClick={() =>
                                                setIconPickerOpen(
                                                    (open) => !open,
                                                )
                                            }
                                        >
                                            {selectedStockIcon ? (
                                                <img
                                                    src={
                                                        selectedStockIcon.preview
                                                    }
                                                    alt=""
                                                />
                                            ) : driveIcon === "custom" &&
                                              customDriveIconPreview ? (
                                                <img
                                                    src={customDriveIconPreview}
                                                    alt=""
                                                />
                                            ) : driveIcon === "bifrost" ? (
                                                <Cloud size={25} />
                                            ) : (
                                                <HardDrive size={25} />
                                            )}
                                            <span>
                                                {selectedDriveIconLabel}
                                            </span>
                                        </button>
                                        {iconPickerOpen && (
                                            <div
                                                className="icon-picker-popover"
                                                role="dialog"
                                                aria-label="Choose drive icon"
                                            >
                                                <div className="icon-picker-tabs">
                                                    <button
                                                        type="button"
                                                        className={
                                                            iconPickerTab ===
                                                            "windows"
                                                                ? "active"
                                                                : ""
                                                        }
                                                        onClick={() =>
                                                            setIconPickerTab(
                                                                "windows",
                                                            )
                                                        }
                                                    >
                                                        Windows
                                                    </button>
                                                    <button
                                                        type="button"
                                                        className={
                                                            iconPickerTab ===
                                                            "bifrost"
                                                                ? "active"
                                                                : ""
                                                        }
                                                        onClick={() =>
                                                            setIconPickerTab(
                                                                "bifrost",
                                                            )
                                                        }
                                                    >
                                                        Bifrost
                                                    </button>
                                                    <button
                                                        type="button"
                                                        className={
                                                            iconPickerTab ===
                                                            "custom"
                                                                ? "active"
                                                                : ""
                                                        }
                                                        onClick={() =>
                                                            setIconPickerTab(
                                                                "custom",
                                                            )
                                                        }
                                                    >
                                                        Custom
                                                    </button>
                                                </div>
                                                <div className="icon-picker-grid">
                                                    {iconPickerTab ===
                                                        "windows" && (
                                                        <>
                                                            <button
                                                                className={`icon-choice ${driveIcon === "system" ? "selected" : ""}`}
                                                                type="button"
                                                                onClick={() => {
                                                                    setDriveIcon(
                                                                        "system",
                                                                    );
                                                                    setIconPickerOpen(
                                                                        false,
                                                                    );
                                                                }}
                                                            >
                                                                <HardDrive
                                                                    size={32}
                                                                />
                                                                <span>
                                                                    System
                                                                    default
                                                                </span>
                                                            </button>
                                                            {stockDriveIcons.map(
                                                                (icon) => (
                                                                    <button
                                                                        className={`icon-choice ${selectedStockIcon?.value === icon.value ? "selected" : ""}`}
                                                                        type="button"
                                                                        key={
                                                                            icon.value
                                                                        }
                                                                        onClick={() => {
                                                                            setDriveIcon(
                                                                                icon.value,
                                                                            );
                                                                            setIconPickerOpen(
                                                                                false,
                                                                            );
                                                                        }}
                                                                    >
                                                                        <img
                                                                            src={
                                                                                icon.preview
                                                                            }
                                                                            alt=""
                                                                        />
                                                                        <span>
                                                                            {
                                                                                icon.label
                                                                            }
                                                                        </span>
                                                                    </button>
                                                                ),
                                                            )}
                                                        </>
                                                    )}
                                                    {iconPickerTab ===
                                                        "bifrost" && (
                                                        <button
                                                            className={`icon-choice ${driveIcon === "bifrost" ? "selected" : ""}`}
                                                            type="button"
                                                            onClick={() => {
                                                                setDriveIcon(
                                                                    "bifrost",
                                                                );
                                                                setIconPickerOpen(
                                                                    false,
                                                                );
                                                            }}
                                                        >
                                                            <Cloud size={32} />
                                                            <span>Bifrost</span>
                                                        </button>
                                                    )}
                                                    {iconPickerTab ===
                                                        "custom" && (
                                                        <button
                                                            className={`icon-choice ${driveIcon === "custom" ? "selected" : ""}`}
                                                            type="button"
                                                            onClick={async () => {
                                                                await handleBrowseDriveIcon();
                                                                setIconPickerOpen(
                                                                    false,
                                                                );
                                                            }}
                                                        >
                                                            {customDriveIconPreview ? (
                                                                <img
                                                                    src={
                                                                        customDriveIconPreview
                                                                    }
                                                                    alt=""
                                                                />
                                                            ) : (
                                                                <FolderOpen
                                                                    size={32}
                                                                />
                                                            )}
                                                            <span>
                                                                Browse custom
                                                            </span>
                                                        </button>
                                                    )}
                                                </div>
                                            </div>
                                        )}
                                    </div>
                                </div>
                                {driveIcon === "custom" && (
                                    <div className="icon-source-field">
                                        <label>
                                            Custom icon source
                                            <input
                                                value={customDriveIcon}
                                                readOnly
                                                required
                                                placeholder="Choose an image or Windows icon source"
                                            />
                                        </label>
                                        <button
                                            className="secondary-button"
                                            type="button"
                                            onClick={handleBrowseDriveIcon}
                                        >
                                            Browse
                                        </button>
                                    </div>
                                )}
                            </div>
                            {filesystemIntegration === "linux" && (
                                <div className="mount-root-field">
                                    <label>
                                        Mount parent folder
                                        <input
                                            name="mountRoot"
                                            value={mountRoot}
                                            onChange={(event) =>
                                                setMountRoot(event.target.value)
                                            }
                                            required
                                        />
                                    </label>
                                    <button
                                        className="secondary-button"
                                        type="button"
                                        onClick={handleBrowseMountRoot}
                                    >
                                        <FolderOpen size={15} /> Browse
                                    </button>
                                </div>
                            )}
                            {filesystemIntegration !== "none" && (
                                <div className="checkbox-row">
                                    <input
                                        name="mountOnStartup"
                                        type="checkbox"
                                        aria-label="Mount this location when Bifrost starts"
                                        defaultChecked={
                                            formDefaults.mountOnStartup !==
                                            false
                                        }
                                    />
                                    Mount this location when Bifrost starts
                                </div>
                            )}
                            {providerChoice !== "SFTP" &&
                                providerChoice !== "GoogleDrive" &&
                                providerChoice !== "GooglePhotos" && (
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
                                    <CustomSelect
                                        label="Authentication"
                                        name="authentication"
                                        value={sftpAuthentication}
                                        options={[
                                            {
                                                value: "password",
                                                label: "Password",
                                                icon: <LockKeyhole size={18} />,
                                            },
                                            {
                                                value: "private_key",
                                                label: "Private key",
                                                icon: <KeyRound size={18} />,
                                            },
                                        ]}
                                        onChange={setSftpAuthentication}
                                    />
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
                                    <div className="checkbox-row">
                                        <input
                                            name="trustOnFirstUse"
                                            type="checkbox"
                                            aria-label="Trust a new server key on first use"
                                            defaultChecked={Boolean(
                                                editingConnection
                                                    ? formDefaults.trustOnFirstUse
                                                    : true,
                                            )}
                                        />
                                        Trust a new server key on first use
                                    </div>
                                </>
                            )}
                            {providerChoice !== "S3" &&
                                providerChoice !== "GoogleDrive" &&
                                providerChoice !== "GooglePhotos" && (
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
                                            sftpAuthentication ===
                                                "password") && (
                                            <label>
                                                Password
                                                <input
                                                    name="password"
                                                    required={
                                                        !editingConnection
                                                    }
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
                            {providerChoice === "GoogleDrive" && (
                                <>
                                    <label className="checkbox-row settings-option">
                                        <input
                                            name="openWorkspaceInNativeApps"
                                            type="checkbox"
                                            defaultChecked={
                                                formDefaults.openWorkspaceInNativeApps !==
                                                false
                                            }
                                        />
                                        <span>
                                            Open Google Workspace files in OS
                                            native apps
                                        </span>
                                    </label>
                                    <button
                                        className="secondary-button"
                                        type="button"
                                        disabled={googleSigningIn}
                                        onClick={() =>
                                            void handleGoogleSignIn()
                                        }
                                    >
                                        {googleSigningIn
                                            ? "Waiting for Google..."
                                            : "Sign in with Google"}
                                    </button>
                                    {googleAuthorization && (
                                        <p className="inline-success">
                                            Google account connected. You can
                                            save this connection.
                                        </p>
                                    )}
                                    <p className="legal-notice">
                                        Bifrost accesses Google Drive files,
                                        folders, metadata, and content to
                                        provide browsing, mounting,
                                        synchronization, and requested file
                                        operations. Native-app mode temporarily
                                        exports Workspace files for local Office
                                        applications. Read the{" "}
                                        <a
                                            href={PRIVACY_POLICY_URL}
                                            target="_blank"
                                            rel="noreferrer"
                                        >
                                            Privacy Policy
                                        </a>
                                        .
                                    </p>
                                </>
                            )}
                            {providerChoice === "GooglePhotos" && (
                                <>
                                    <label>
                                        Legacy Google Drive folder ID
                                        <input
                                            name="legacyFolderId"
                                            defaultValue={
                                                formDefaults.legacyFolderId as string
                                            }
                                            placeholder="Optional: old Google Photos archive folder ID"
                                        />
                                    </label>
                                    <label>
                                        Legacy Google Drive folder path
                                        <input
                                            name="legacyFolderPath"
                                            defaultValue={
                                                (formDefaults.legacyFolderPath as string) ??
                                                "Fritid/Google Foto"
                                            }
                                            placeholder="Fritid/Google Foto"
                                        />
                                    </label>
                                    <button
                                        className="secondary-button"
                                        type="button"
                                        disabled={googleSigningIn}
                                        onClick={() =>
                                            void handleGoogleSignIn()
                                        }
                                    >
                                        {googleSigningIn
                                            ? "Waiting for Google..."
                                            : "Sign in with Google"}
                                    </button>
                                    {googleAuthorization && (
                                        <p className="inline-success">
                                            Google account connected. You can
                                            save this connection.
                                        </p>
                                    )}
                                    <p className="legal-notice">
                                        Bifrost accesses only media and albums
                                        created by Bifrost through the official
                                        Google Photos API. Optionally, a legacy
                                        Google Drive folder appears under
                                        Legacy and retains Google Drive file
                                        behavior. Files you add count toward
                                        your Google storage. Read the{" "}
                                        <a
                                            href={PRIVACY_POLICY_URL}
                                            target="_blank"
                                            rel="noreferrer"
                                        >
                                            Privacy Policy
                                        </a>
                                        .
                                    </p>
                                </>
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
                                <div className="checkbox-row">
                                    <input
                                        name="pathStyle"
                                        type="checkbox"
                                        aria-label="Use path-style addressing"
                                        defaultChecked={Boolean(
                                            formDefaults.pathStyle,
                                        )}
                                    />
                                    Use path-style addressing
                                </div>
                            )}
                            <div className="wizard-actions">
                                <button
                                    className="secondary-button"
                                    type="button"
                                    onClick={closeWizard}
                                >
                                    Cancel
                                </button>
                                <button
                                    className="primary-button"
                                    disabled={
                                        saving ||
                                        ((providerChoice === "GoogleDrive" ||
                                            providerChoice === "GooglePhotos") &&
                                            !googleAuthorization &&
                                            !editingConnection)
                                    }
                                    type="submit"
                                >
                                    {saving
                                        ? "Connecting..."
                                        : editingConnection
                                          ? "Mount drive changes"
                                          : "Mount drive"}
                                </button>
                            </div>
                        </form>
                    </section>
                </section>
            )}
            {updatePopupOpen && updateInfo && (
                <div className="update-modal-backdrop">
                    <section
                        className="update-modal"
                        role="dialog"
                        aria-modal="true"
                        aria-labelledby="update-modal-title"
                    >
                        <div className="section-heading">
                            <div>
                                <p className="eyebrow">Software update</p>
                                <h2 id="update-modal-title">
                                    Version {updateInfo.version} is available
                                </h2>
                            </div>
                            <button
                                className="icon-button"
                                type="button"
                                aria-label="Close update"
                                onClick={() => setUpdatePopupOpen(false)}
                            >
                                x
                            </button>
                        </div>
                        <p>Would you like to install it now?</p>
                        <ReleaseNotes body={updateInfo.body} />
                        <div className="wizard-actions">
                            <button
                                className="secondary-button"
                                type="button"
                                onClick={() => setUpdatePopupOpen(false)}
                            >
                                No, later
                            </button>
                            <button
                                className="primary-button"
                                type="button"
                                disabled={installingUpdate}
                                onClick={() => void handleInstallUpdate()}
                            >
                                {installingUpdate
                                    ? "Installing..."
                                    : "Yes, update now"}
                            </button>
                        </div>
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
        explorer_opened: "Opened filesystem location",
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
        case "GoogleDrive":
            return "GoogleDrive";
        case "GooglePhotos":
            return "GooglePhotos";
    }
}
