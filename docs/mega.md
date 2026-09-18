# MEGA

The MEGA provider uses the official MEGA API client flow with client-side encryption support. The desktop wizard accepts a MEGA account email, password, and optional start path. The start path defaults to `/` (the MEGA cloud drive root); set a subdirectory to scope the mount.

Credentials are stored in the native credential service. The provider supports authenticated node listing, reads, writes, deletion, renaming, directory creation, and storage quota reporting. Two-factor authentication is not currently exposed by the connection wizard.

Live MEGA integration tests require a test account and are not run by default. Do not place real credentials in repository files or test fixtures.