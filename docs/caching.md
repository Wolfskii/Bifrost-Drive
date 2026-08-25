# Caching

The cache filesystem primitive is implemented. It keeps remote content under stable hashed identities, commits temporary downloads atomically, supports pin boundaries, enforces maximum size with LRU eviction, and never evicts pinned or active-transfer content. Cache metadata is restored from SQLite on startup, and provider hydration is connected to the transfer service. Native Explorer VM acceptance remains outstanding.

Content is written to a temporary path and atomically committed. Cache paths derive from stable connection and metadata identifiers, not unchecked remote strings. SQLite stores cache metadata, not file contents.
