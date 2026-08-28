# Google Drive Setup

## One-time Google Cloud setup

1. Open [Google Cloud Console](https://console.cloud.google.com/).
2. Create a project, or select an existing project for Bifrost.
3. Open **APIs & Services > Library**, search for **Google Drive API**, and click **Enable**.
4. Open **APIs & Services > OAuth consent screen**.
5. Choose **External** for personal accounts, or **Internal** for a Google Workspace organization.
6. Add the users who will test the app if Google shows a **Test users** section.
7. Open **APIs & Services > Credentials > Create credentials > OAuth client ID**.
8. Choose **Desktop app**, create it, and copy the client ID.
9. In the GitHub repository, open **Settings > Secrets and variables > Actions > Variables** and add `BIFROST_GOOGLE_OAUTH_CLIENT_ID` with that client ID.
10. Build and publish Bifrost again. The client ID is compiled into the app so users do not need to enter it.

## Connect Bifrost

1. Open Bifrost and click **Add connection**.
2. Select **Google Drive**.
3. Click **Sign in with Google** and finish the browser consent screen.
4. Return to Bifrost and click **Test and save**.
5. To use a Shared Drive, paste its ID into **Shared Drive ID**. Leave it blank for My Drive.

Bifrost opens a temporary localhost callback for the sign-in. The authorization code is protected with PKCE and the state value is checked before tokens are accepted.

## What Bifrost stores

The access token and refresh token are stored in the native Windows Credential Manager, macOS Keychain, or Linux Secret Service. SQLite stores only the connection configuration and optional Shared Drive ID. User tokens never pass through GitHub Actions.

The OAuth client ID is public by design: every installed desktop app must send it to Google during sign-in. Store it as the GitHub Actions repository variable `BIFROST_GOOGLE_OAUTH_CLIENT_ID`, not as a GitHub secret. Never store a user access token, refresh token, or OAuth client secret in GitHub.

Bifrost refreshes the access token automatically when it expires. The refresh token normally remains valid until the user revokes access or the Google Cloud OAuth client is changed.

## Supported operations

The provider supports listing, metadata, streaming reads and writes, range reads, folder creation, rename, server-side copy, deletion, and storage quota reporting. Shared Drive operations use Google’s `supportsAllDrives` and `includeItemsFromAllDrives` settings.

Google Workspace editor files that cannot be downloaded as binary media are not readable through this provider yet. A Google OAuth app in testing may also require the account to be listed as a test user.

## Troubleshooting

- **Access blocked:** add the Google account under the OAuth consent screen’s test users, or publish/verify the OAuth app as required by Google.
- **Sign-in timed out:** start the sign-in again and allow Bifrost through local firewall prompts.
- **Refresh failed:** remove the connection, revoke its Bifrost permission in the Google account, and connect again.
- **Shared Drive is empty:** check the Shared Drive ID and confirm the Google account has access to that drive.
