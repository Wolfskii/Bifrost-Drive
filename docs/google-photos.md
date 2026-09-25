# Google Photos Setup

## Google Cloud setup

1. Select the existing Bifrost Google Cloud project.
2. Enable the Google Photos Library API under **APIs & Services > Library**.
3. Add these scopes to the OAuth consent screen:
   - `https://www.googleapis.com/auth/photoslibrary.appendonly`
   - `https://www.googleapis.com/auth/photoslibrary.readonly.appcreateddata`
   - `https://www.googleapis.com/auth/photoslibrary.edit.appcreateddata`
4. Submit the updated consent screen for verification before public distribution.

Bifrost uses the existing Desktop OAuth client ID and client secret. A Google Photos connection runs its own consent flow and stores its access and refresh tokens only in the native credential store.

## Current capabilities

The official Google Photos Library API exposes only content created by Bifrost. The mounted connection contains:

- `All Photos`: media uploaded through Bifrost.
- `Albums`: albums created through Bifrost and their Bifrost-created media.

Bifrost can upload supported media, list and stream Bifrost-created media, create albums, and rename Bifrost-created albums. Uploaded media is stored at original quality and counts toward the Google account's storage quota.

Listed media items are cached for 50 minutes, inside Google's 60-minute base URL lifetime, and the album list for five minutes. `All Photos` items are fetched directly by the media ID embedded in their virtual name instead of paging through the library. Album items that are not cached fall back to an album search. Rate-limit and `5xx` responses are retried with capped exponential backoff.

Google does not offer an official API to delete media items, empty Google Photos trash, or delete album containers. Bifrost therefore rejects deletion through this official connection.

## Planned hybrid connection

The planned hybrid connection will add a separately disclosed experimental Google Photos web-session bridge for full-library browsing and moving media to trash, plus an optional scoped legacy Google Drive archive. It will never store browser cookies, CSRF values, or page session tokens in SQLite or logs. Until that work is live and verified, Bifrost does not claim full-library Google Photos access.
