#!/bin/bash
set -e

cleanup() {
    docker compose down -v --remove-orphans
}
trap cleanup EXIT

docker compose up -d --build

docker compose exec test-runner bash ./tests/test_script.sh
docker compose exec test-runner bash ./tests/test_edge_cases.sh
docker compose exec test-runner bash ./tests/test_container_lifecycle.sh
docker compose exec test-runner bash ./tests/test_multi_destination.sh
docker compose exec test-runner bash ./tests/test_interrupt.sh