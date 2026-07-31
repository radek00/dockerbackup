docker compose up -d --build

docker compose exec test-runner bash ./tests/test_script.sh

docker compose down -v --remove-orphans