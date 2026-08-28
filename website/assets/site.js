import {
    detectPlatform,
    normalizeGitHubRelease,
    RELEASE_METADATA,
    RELEASE_PAGE,
    resolveDownload,
} from "./download-resolver.js";

const downloadLinks = [...document.querySelectorAll("[data-download-link]")];
const downloadLabels = [...document.querySelectorAll("[data-download-label]")];
const downloadDetails = [
    ...document.querySelectorAll("[data-download-detail]"),
];
const versionLabels = [...document.querySelectorAll("[data-release-version]")];
const platform = detectPlatform(navigator.userAgent, navigator.platform);

function applyDownload(result) {
    downloadLinks.forEach((link) => {
        link.href = result.url;
        link.dataset.status = result.status;
        link.setAttribute("aria-label", `${result.label}. ${result.detail}`);
    });
    downloadLabels.forEach((label) => {
        label.textContent = result.label;
    });
    downloadDetails.forEach((detail) => {
        detail.textContent = result.detail;
    });
    versionLabels.forEach((label) => {
        label.textContent = result.version;
    });
}

applyDownload(resolveDownload(platform, null));

fetch(RELEASE_METADATA, {
    cache: "no-store",
    headers: { Accept: "application/vnd.github+json" },
})
    .then((response) => {
        if (!response.ok)
            throw new Error(`Release metadata returned ${response.status}`);
        return response.json();
    })
    .then((release) =>
        applyDownload(
            resolveDownload(platform, normalizeGitHubRelease(release)),
        ),
    )
    .catch(() => applyDownload(resolveDownload(platform, null)));

document.querySelectorAll("[data-menu-toggle]").forEach((button) => {
    button.addEventListener("click", () => {
        const navigation = document.querySelector("[data-navigation]");
        const expanded = button.getAttribute("aria-expanded") === "true";
        button.setAttribute("aria-expanded", String(!expanded));
        button.setAttribute(
            "aria-label",
            expanded ? "Open navigation" : "Close navigation",
        );
        navigation?.toggleAttribute("data-open", !expanded);
        document.body.classList.toggle("menu-open", !expanded);
    });
});

document.querySelectorAll("[data-navigation] a").forEach((link) => {
    link.addEventListener("click", () => {
        document
            .querySelector("[data-navigation]")
            ?.removeAttribute("data-open");
        document
            .querySelector("[data-menu-toggle]")
            ?.setAttribute("aria-expanded", "false");
        document
            .querySelector("[data-menu-toggle]")
            ?.setAttribute("aria-label", "Open navigation");
        document.body.classList.remove("menu-open");
    });
});
document.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    document.querySelector("[data-navigation]")?.removeAttribute("data-open");
    const button = document.querySelector("[data-menu-toggle]");
    button?.setAttribute("aria-expanded", "false");
    button?.setAttribute("aria-label", "Open navigation");
    document.body.classList.remove("menu-open");
    button?.focus();
});

window.addEventListener("error", () => {
    downloadLinks.forEach((link) => {
        if (!link.href) link.href = RELEASE_PAGE;
    });
});
