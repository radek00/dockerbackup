FROM rust:1.93.1-trixie AS builder

WORKDIR /app

COPY Cargo.toml Cargo.lock* ./

COPY src ./src

RUN cargo build --release

FROM debian:trixie-20260223-slim

RUN apt-get update && apt-get install -y \
    docker.io \
    openssh-client \
    rsync \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user for running the backup
# Remember to set the same UID as the host user
RUN useradd -u 10000 -m -s /bin/bash dockerbackup

# Create directories for volumes and backup destination
# /volumes - mount point for Docker volumes to backup
# /backup - local backup destination (if not using SSH)
# /ssh - optional SSH configuration directory
RUN mkdir -p /volumes/backingFsBlockDev /backup /ssh && \
    chown 10000:10000 /volumes /backup /ssh

WORKDIR /app

COPY --from=builder /app/target/release/dockerbackup /usr/local/bin/dockerbackup

COPY --chmod=755 <<'EOF' /usr/local/bin/docker-entrypoint.sh
#!/bin/sh
set -e

if [ -d "/ssh" ] && [ "$(ls -A /ssh 2>/dev/null)" ]; then
    echo "Setting up SSH configuration..."
    
    USER_HOME=$(eval echo ~)

    cp -r /ssh "$USER_HOME/.ssh"
    
    chmod 700 "$USER_HOME/.ssh"

    find "$USER_HOME/.ssh" -type f -exec chmod 600 {} \;
    
    echo "SSH configuration ready"
fi

CMD="dockerbackup"

DEST="${DESTINATION:-/backup}"
for dest in $DEST; do
    CMD="$CMD --destination $dest"
done

CMD="$CMD --volumes /volumes"

if [ -n "$EXCLUDED_CONTAINERS" ]; then
    for container in $EXCLUDED_CONTAINERS; do
        CMD="$CMD --exclude-containers $container"
    done
fi

if [ -n "$EXCLUDED_VOLUMES" ]; then
    for volume in $EXCLUDED_VOLUMES; do
        CMD="$CMD --exclude-volumes $volume"
    done
fi

if [ -n "$GOTIFY_URL" ]; then
    CMD="$CMD --gotify $GOTIFY_URL"
fi

if [ -n "$DISCORD_URL" ]; then
    CMD="$CMD --discord $DISCORD_URL"
fi

exec $CMD "$@"
EOF

USER 10000

ENV EXCLUDED_CONTAINERS="" \
    EXCLUDED_VOLUMES="" \
    GOTIFY_URL="" \
    DISCORD_URL=""


ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]

