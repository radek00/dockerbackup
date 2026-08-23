#!/bin/bash
GREEN='\033[0;32m'
NC='\033[0m'
set -e

cleanup() {
    docker compose down -v --remove-orphans
}
trap cleanup EXIT

docker compose up -d --build

docker compose exec test-runner bash ./tests/test_script.sh

docker compose exec test-runner bash ./tests/test_edge_cases.sh
echo -e "${GREEN}All edge cases tests passed!${NC}"

docker compose exec test-runner bash ./tests/test_container_lifecycle.sh
echo -e "${GREEN}All container lifecycle tests passed!${NC}"

docker compose exec test-runner bash ./tests/test_multi_destination.sh
echo -e "${GREEN}All multi-destination tests passed!${NC}"

docker compose exec test-runner bash ./tests/test_interrupt.sh
echo -e "${GREEN}All interrupt tests passed!${NC}"

docker compose exec test-runner bash ./tests/test_restore.sh
echo -e "${GREEN}All restore tests passed!${NC}"

echo -e "${GREEN}All tests passed!${NC}"