#!/usr/bin/env bash
if [ -f .env ]; then
  export $(grep -v '^#' .env | xargs)
fi

# Function to run docker compose up
# If METRICS_ENABLED is true, run docker compose up with the metrics profile
docker_compose_up() {
  if [ "$METRICS_ENABLED" = "true" ]; then
    docker compose --profile metrics up -d
  else
    docker compose up -d
  fi
}

# Function to run docker compose down
# If METRICS_ENABLED is true, run docker compose down with the metrics profile
docker_compose_down() {
  if [ "$METRICS_ENABLED" = "true" ]; then
    docker compose --profile metrics down
  else
    docker compose down
  fi
}

# Check command-line argument
case "$1" in
  up)
    docker_compose_up
    ;;
  down)
    docker_compose_down
    ;;
  *)
    echo "Usage: $0 {up|down}"
    exit 1
    ;;
esac
