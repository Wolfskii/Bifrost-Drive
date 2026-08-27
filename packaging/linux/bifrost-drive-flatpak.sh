#!/bin/sh
export APPDIR=/app/bifrost
export WEBKIT_DISABLE_DMABUF_RENDERER="${WEBKIT_DISABLE_DMABUF_RENDERER:-1}"
exec /app/bifrost/AppRun "$@"