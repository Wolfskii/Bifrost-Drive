# Google Drive Setup

## One-time Google Cloud setup

1. Open [Google Cloud Console](https://console.cloud.google.com/).
2. Create a project, or select an existing project for Bifrost.
3. Open **APIs & Services > Library**, search for **Google Drive API**, and click **Enable**.
4. Open **APIs & Services > OAuth consent screen**.
5. Choose **External** for personal accounts, or **Internal** for a Google Workspace organization.
6. Add the users who will test the app if Google shows a **Test users** section.
7. Under **Data Access**, add `https://www.googleapis.com/auth/drive`. This is a restricted scope and requires Google's restricted-scope verification before a public production release.
8. Open **APIs & Services > Credentials > Create credentials > OAuth client ID**.
9. Choose **Desktop app**, create it, and copy the client ID and generated client secret.
10. In the GitHub repository, open **Settings > Secrets and variables > Actions > Variables** and add `BIFROST_GOOGLE_OAUTH_CLIENT_ID` with the client ID.
11. Under **Settings > Secrets and variables > Actions > Secrets**, add `BIFROST_GOOGLE_OAUTH_CLIENT_SECRET` with the generated client secret.
12. Build and publish Bifrost again. Both values are compiled into the app so users do not need to enter them.

## Connect Bifrost

1. Open Bifrost and click **Add connection**.
2. Select **Google Drive**.
3. Click **Sign in with Google** and finish the browser consent screen.
4. Return to Bifrost and click **Test and save**.
5. To use a Shared Drive, paste its ID into **Shared Drive ID**. Leave it blank for My Drive.

Bifrost opens a temporary localhost callback for the sign-in. The authorization code is protected with PKCE and the state value is checked before tokens are accepted.

## What Bifrost stores

The access token and refresh token are stored in the native Windows Credential Manager, macOS Keychain, or Linux Secret Service. SQLite stores only the connection configuration and optional Shared Drive ID. User tokens never pass through GitHub Actions.

The OAuth client ID is public by design: every installed desktop app must send it to Google during sign-in. Store it as the GitHub Actions repository variable `BIFROST_GOOGLE_OAUTH_CLIENT_ID`. Store the generated Desktop client secret as the GitHub Actions secret `BIFROST_GOOGLE_OAUTH_CLIENT_SECRET`, and never commit it. Because installed applications cannot keep embedded values confidential, this client secret must not be treated as proof that a request came from an authentic Bifrost installation. Never store user access or refresh tokens in GitHub.

Bifrost refreshes the access token automatically when it expires. The refresh token normally remains valid until the user revokes access or the Google Cloud OAuth client is changed.

## Supported operations

The provider supports listing, metadata, streaming reads and writes, range reads, folder creation, rename, server-side copy, deletion, and storage quota reporting. Shared Drive operations use Google’s `supportsAllDrives` and `includeItemsFromAllDrives` settings.

Google Workspace files appear as virtual Microsoft Office files so desktop applications can open them:

- Google Docs use `.docx`.
- Google Sheets use `.xlsx`.
- Google Slides use `.pptx`.

Bifrost exports the document on open and imports edited Office content into the same native Google file on save. Google limits exported Workspace documents to 10 MB. Export and import can lose features that do not have an equivalent in both formats. Bifrost refuses save-back when the Google file version changed after it was opened, preventing an edited Office copy from silently overwriting newer online changes.

The connection setting **Open Google Workspace files in OS native apps** is enabled by default. Disable it to expose Docs, Sheets, and Slides as read-only `.url` shortcuts instead; opening a shortcut launches the document's Google editor link in the default browser. Browser shortcuts are never imported into Google Drive.

Bifrost caches resolved Google Drive directory IDs for the lifetime of the connection, avoiding repeated API walks for every file operation in deep folders. Directory-changing mutations invalidate the cache. My Drive searches use the narrower user corpus; shared drives retain drive-scoped queries.

Opening and closing an Office alias without changing it does not upload or convert anything. Bifrost stages the exported file locally and only imports it after the operating system reports a content write, so the native Google document remains unchanged on a read-only open.

To keep virtual names unambiguous, an ordinary uploaded file whose name already ends in `.docx`, `.xlsx`, or `.pptx` is displayed with its final dot encoded as `%2E`. Other native Google Workspace types remain metadata-only until an editable export format is implemented. A Google OAuth app in testing may also require the account to be listed as a test user.

## Troubleshooting

- **Access blocked:** add the Google account under the OAuth consent screen’s test users, or publish/verify the OAuth app as required by Google.
- **`client_secret is missing`:** add `BIFROST_GOOGLE_OAUTH_CLIENT_SECRET` to GitHub Actions Secrets and rebuild the application.
- **Sign-in timed out:** start the sign-in again and allow Bifrost through local firewall prompts.
- **Refresh failed:** remove the connection, revoke its Bifrost permission in the Google account, and connect again.
- **Shared Drive is empty:** check the Shared Drive ID and confirm the Google account has access to that drive.
