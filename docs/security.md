# Security

Security requirements apply from the first provider implementation: native credential storage, TLS validation, SSH host verification, safe path handling, restrictive cache/temp permissions, atomic writes, Tauri CSP and capability minimization, authenticated local IPC, dependency auditing, and signed/verified updates.

No proprietary encryption format is used for normal connections. Cryptomator-compatible vault support is future work and must prioritize interoperability if implemented.
