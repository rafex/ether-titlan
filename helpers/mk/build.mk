.PHONY: wasm frontend-check backend-check check build container ci

ROOT_DIR := $(abspath $(dir $(lastword $(MAKEFILE_LIST)))../..)

wasm:
	@$(ROOT_DIR)/helpers/shell/build_wasm.sh

frontend-check:
	@$(ROOT_DIR)/helpers/shell/check_frontend.sh

backend-check:
	@$(ROOT_DIR)/helpers/shell/check_backend.sh

check: frontend-check backend-check

build: wasm check

container: build
	@$(ROOT_DIR)/helpers/shell/build_image.sh

ci:
	@$(ROOT_DIR)/helpers/shell/build_ci.sh
