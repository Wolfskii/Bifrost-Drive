# Caching

The cache filesystem primitive is implemented. It keeps remote content under stable hashed identities, commits temporary downloads atomically, supports pin boundaries, enforces maximum size with LRU eviction, and never evicts pinned or active-transfer content. Durable cache metadata tables are present; restart reconciliation and CFAPI hydration integration remain Planned.

Content is written to a temporary path and atomically committed. Cache paths derive from stable connection and metadata identifiers, not unchecked remote strings. SQLite stores cache metadata, not file contents.
