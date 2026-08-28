# Google Drive

Google Drive connections use the Drive v3 REST API over HTTPS. The desktop connection wizard accepts a Google OAuth access token and stores it in the native credential store; the SQLite database stores only the connection endpoint and provider-neutral mount settings.

Use the default endpoint `https://www.googleapis.com/drive/v3`. The token must include the Drive scope needed for the operations you want Bifrost to perform. Bifrost verifies the token with `about.get` before saving the connection.

The provider supports listing, metadata, streaming reads and writes, range reads, folder creation, rename, server-side copy, deletion, and storage quota reporting. Google Workspace editor files that cannot be downloaded as binary media are not readable through this provider yet.

Access tokens are short-lived. Token refresh and the interactive browser OAuth flow remain planned; create a new connection or update the token before it expires.
