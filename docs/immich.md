# Immich

Bifrost can connect to a self-hosted Immich server and expose its photos and albums as a read-only remote filesystem.

## Connection

Enter the Immich server URL and choose one authentication mode:

- **API key** sends the key in Immich's `x-api-key` header.
- **Email and password** logs in through Immich and uses the returned session token.

The API key, email, password, and session token are kept in the native credential store. SQLite stores only the connection endpoint, authentication mode, and mount configuration.

The URL may include a reverse-proxy path. `https://` or `http://` may be omitted. For a URL without a scheme, Bifrost tests HTTPS first and then HTTP if the HTTPS probe cannot complete. An explicitly entered HTTPS URL is never downgraded. Use HTTP only on a trusted local network because credentials and API keys are not encrypted by HTTP.

The connection is saved only after an authenticated request to the Immich API succeeds. The confirmed URL, including its selected scheme and proxy path, is saved with the connection.

## Filesystem layout

The root contains:

- `Photos`, containing all assets returned by Immich metadata search.
- `Albums`, containing Immich albums and their assets.

Asset names include the original filename and Immich asset ID so duplicate filenames remain addressable. Original asset downloads are streamed and range reads are supported.

Immich connections are read-only in this release. Creating, deleting, renaming, copying, moving, and replacing remote items are unsupported.