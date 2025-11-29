#!/bin/bash
set -e

echo "Waiting for Docker daemon..."
until docker info > /dev/null 2>&1; do
  sleep 1
done
echo "Docker is ready."

echo "Setting up SSH access..."
mkdir -p /ssh_config/.ssh
cat /root/.ssh/id_rsa.pub >> /ssh_config/.ssh/authorized_keys
chmod 600 /ssh_config/.ssh/authorized_keys

# Wait for SSH to be ready
echo "Waiting for SSH target..."
until ssh -o StrictHostKeyChecking=no -o ConnectTimeout=5 testuser@ssh-target echo "SSH ready"; do
  echo "Retrying SSH connection..."
  sleep 2
done
echo "SSH is ready."

# Create remote backup directory to ensure df checks pass
ssh -o StrictHostKeyChecking=no testuser@ssh-target "mkdir -p /config/remote_backup"

echo "Cleaning up old containers and volumes..."
docker rm -f container1 container2 container_excluded || true
docker volume rm backup_test_vol1 backup_test_vol2 backup_test_vol_excluded || true

echo "Creating dummy Docker volumes and containers..."
# Create volumes
docker volume create backup_test_vol1
docker volume create backup_test_vol2
docker volume create backup_test_vol_excluded

# Populate volumes
docker run --rm -v backup_test_vol1:/data alpine sh -c "echo 'Hello World' > /data/file1.txt"
docker run --rm -v backup_test_vol2:/data alpine sh -c "echo 'Important Data' > /data/file2.txt"
docker run --rm -v backup_test_vol_excluded:/data alpine sh -c "echo 'Skip Me' > /data/file3.txt"

# Run containers
docker run -d --name container1 -v backup_test_vol1:/data alpine sleep infinity
docker run -d --name container2 -v backup_test_vol2:/data alpine sleep infinity
# Excluded container
docker run -d --name container_excluded -v backup_test_vol_excluded:/data alpine sleep infinity

echo "Environment setup complete."
