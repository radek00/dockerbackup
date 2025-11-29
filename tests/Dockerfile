FROM rust:latest

# Install dependencies
RUN apt-get update && apt-get install -y \
    docker.io \
    rsync \
    openssh-client \
    sshpass \
    && rm -rf /var/lib/apt/lists/*

# Create a directory for the app
WORKDIR /app

# Generate SSH keys
RUN mkdir -p /root/.ssh && \
    ssh-keygen -t rsa -b 4096 -f /root/.ssh/id_rsa -N "" && \
    chmod 600 /root/.ssh/id_rsa

# Configure SSH to ignore host key checking for testing and set port for ssh-target
RUN echo "Host ssh-target\n\tPort 2222\n\nHost *\n\tStrictHostKeyChecking no\n\tUserKnownHostsFile=/dev/null" > /root/.ssh/config
