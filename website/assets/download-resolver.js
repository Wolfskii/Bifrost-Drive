export const RELEASE_PAGE =
    "https://github.com/Wolfskii/Bifrost-Drive/releases/latest";
export const RELEASE_METADATA =
    "https://api.github.com/repos/Wolfskii/Bifrost-Drive/releases/latest";

export function normalizeGitHubRelease(release) {
    const assets = Array.isArray(release?.assets) ? release.assets : [];
    const findUrl = (pattern) =>
        assets.find((asset) => pattern.test(asset?.name ?? ""))
            ?.browser_download_url;

    return {
        version: String(release?.tag_name ?? "").replace(/^v/, ""),
        platforms: {
            "windows-x86_64": {
                url: findUrl(/Bifrost-Drive-Setup-x64\.exe$/i),
            },
            "linux-x86_64": {
                url: findUrl(/\.AppImage$/i),
            },
        },
    };
}

export function detectPlatform(userAgent = "", navigatorPlatform = "") {
    const value = `${userAgent} ${navigatorPlatform}`.toLowerCase();
    if (/iphone|ipad|ipod|android/.test(value)) return "other";
    if (/macintosh|mac os|macintel/.test(value)) return "macos";
    if (/windows|win32|win64/.test(value)) return "windows";
    if (/linux|x11/.test(value)) return "linux";
    return "other";
}

export function resolveDownload(platform, metadata) {
    const version = metadata?.version
        ? `v${metadata.version}`
        : "Latest release";
    const platforms = metadata?.platforms ?? {};
    const platformKey =
        platform === "windows"
            ? "windows-x86_64"
            : platform === "linux"
              ? "linux-x86_64"
              : null;
    const artifact = platformKey ? platforms[platformKey] : null;

    if (platform === "macos") {
        return {
            status: "coming-soon",
            label: "macOS coming soon",
            detail: "Signed macOS distribution is being prepared.",
            url: RELEASE_PAGE,
            version,
        };
    }

    if (artifact?.url) {
        return {
            status: "available",
            label:
                platform === "windows"
                    ? "Download for Windows"
                    : "Download AppImage for Linux",
            detail:
                platform === "windows"
                    ? "Windows 10/11"
                    : "Linux x86_64 · early access",
            url: artifact.url,
            version,
        };
    }

    return {
        status: "fallback",
        label:
            platform === "other"
                ? "View all downloads"
                : `View ${platform === "windows" ? "Windows" : "Linux"} download`,
        detail: "Choose an installer from the latest GitHub release.",
        url: RELEASE_PAGE,
        version,
    };
}
