# Caching

The filesystem cache is **Planned**. It will keep remote metadata available without content downloads, hydrate files on demand, support file/folder/connection pins, enforce maximum size and age/LRU eviction, and never evict pinned or active-transfer content.

Content is written to a temporary path and atomically committed. Cache paths derive from stable connection and metadata identifiers, not unchecked remote strings. SQLite stores cache metadata, not file contents.
