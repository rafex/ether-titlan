set dotenv-load := false

# Operación local: arranca backend Python y frontend Node.js.
dev: build
    ./helpers/shell/start_stack.sh

start: build
    ./helpers/shell/start_stack.sh

stop:
    ./helpers/shell/stop_stack.sh

logs:
    ./helpers/shell/logs.sh

poc-up: container
    ./helpers/shell/poc_up.sh

poc-down:
    ./helpers/shell/poc_down.sh

poc-logs:
    ./helpers/shell/poc_logs.sh

clean:
    ./helpers/shell/clean.sh

# Just puede solicitar artefactos al orquestador de construcción.
build:
    make build

container:
    make container

ci:
    make ci

check:
    make check
