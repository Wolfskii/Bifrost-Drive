import test from "node:test";
import assert from "node:assert/strict";
import {
    detectPlatform,
    normalizeGitHubRelease,
    RELEASE_PAGE,
    resolveDownload,
} from "../assets/download-resolver.js";

const metadata = {
    version: "0.3.0",
    platforms: {
        "windows-x86_64": {
            url: "https://downloads.example/Bifrost-Drive-Setup-x64.exe",
        },
        "linux-x86_64": {
            url: "https://downloads.example/Bifrost-Drive.AppImage",
        },
    },
};

test("detects Windows and resolves the direct installer", () => {
    assert.equal(
        detectPlatform("Mozilla/5.0 (Windows NT 10.0; Win64; x64)"),
        "windows",
    );
    assert.deepEqual(resolveDownload("windows", metadata), {
        status: "available",
        label: "Download for Windows",
        detail: "Windows 10/11",
        url: "https://downloads.example/Bifrost-Drive-Setup-x64.exe",
        version: "v0.3.0",
    });
});

test("detects Linux and resolves the direct AppImage", () => {
    assert.equal(detectPlatform("Mozilla/5.0 (X11; Linux x86_64)"), "linux");
    assert.equal(
        resolveDownload("linux", metadata).url,
        "https://downloads.example/Bifrost-Drive.AppImage",
    );
});

test("detects macOS and reports that support is coming soon", () => {
    assert.equal(
        detectPlatform("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)"),
        "macos",
    );
    const result = resolveDownload("macos", metadata);
    assert.equal(result.status, "coming-soon");
    assert.equal(result.label, "macOS coming soon");
    assert.equal(result.url, RELEASE_PAGE);
});

test("uses GitHub Releases when metadata or a platform artifact is unavailable", () => {
    assert.equal(resolveDownload("windows", null).url, RELEASE_PAGE);
    assert.equal(
        resolveDownload("linux", { version: "0.3.0", platforms: {} }).url,
        RELEASE_PAGE,
    );
    assert.equal(
        resolveDownload("other", metadata).label,
        "View all downloads",
    );
});

test("does not classify Android as desktop Linux", () => {
    assert.equal(detectPlatform("Mozilla/5.0 (Linux; Android 14)"), "other");
});

test("normalizes GitHub release assets for direct OS downloads", () => {
    assert.deepEqual(
        normalizeGitHubRelease({
            tag_name: "v0.3.2",
            assets: [
                {
                    name: "Bifrost-Drive-Setup-x64.exe",
                    browser_download_url:
                        "https://downloads.example/windows.exe",
                },
                {
                    name: "Bifrost.Drive_0.3.2_amd64.AppImage",
                    browser_download_url:
                        "https://downloads.example/linux.AppImage",
                },
            ],
        }),
        {
            version: "0.3.2",
            platforms: {
                "windows-x86_64": {
                    url: "https://downloads.example/windows.exe",
                },
                "linux-x86_64": {
                    url: "https://downloads.example/linux.AppImage",
                },
            },
        },
    );
});
