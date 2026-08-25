# FTP and FTPS

The FTP provider supports `ftp://` and explicit `ftps://` endpoints, streaming reads and writes, directory listing through MLSD, metadata through MLST, directory creation, rename, and deletion. FTPS uses rustls with the platform root certificate set; certificate verification is not disabled.

FTP listings are not cursor-paginated because the protocol returns a complete directory listing. Servers without MLSD/MLST support are not silently treated as verified metadata sources. The provider is available through the desktop connection wizard.
