# FTP and FTPS

The FTP provider supports standard FTP and explicit TLS through FTPS. The desktop connection wizard asks for the protocol, host, port, start path, username, and password; it builds the `ftp://` or `ftps://` endpoint from those fields instead of treating FTP as an S3-compatible endpoint. FTPS uses rustls with the platform root certificate set; certificate verification is not disabled.

The optional start path scopes all remote operations below that FTP directory. It may be relative to the account's FTP root or an absolute FTP path, but it cannot contain `..` parent traversal.

FTP listings are not cursor-paginated because the protocol returns a complete directory listing. Servers without MLSD/MLST support are not silently treated as verified metadata sources. The provider is available through the desktop connection wizard.
