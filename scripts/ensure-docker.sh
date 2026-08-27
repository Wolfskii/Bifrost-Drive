#!/usr/bin/env sh
set -eu

if ! command -v docker >/dev/null 2>&1; then
    echo "Docker CLI was not found. Install Docker Desktop or Docker Engine and try again." >&2
    exit 1
fi

if docker info >/dev/null 2>&1; then
    echo "Docker is ready."
    exit 0
fi

case "$(uname -s)" in
    Darwin)
        echo "Starting Docker Desktop..."
        open -a Docker
        ;;
    Linux)
        echo "Starting Docker Desktop..."
        systemctl --user start docker-desktop
        ;;
    *)
        echo "Docker is not running. Start the Docker daemon and try again." >&2
        exit 1
        ;;
esac

attempt=0
while ! docker info >/dev/null 2>&1; do
    attempt=$((attempt + 1))
    if [ "$attempt" -ge 60 ]; then
        echo "Docker did not become ready within two minutes." >&2
        exit 1
    fi
    sleep 2
done

echo "Docker is ready."