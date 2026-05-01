#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH="${POCKET_CONFIG:-$HOME/.pocket-harness/config.yaml}"
DATA_DIR="${POCKET_DATA_DIR:-$HOME/.pocket-harness}"
ENV_FILE="${POCKET_ENV_FILE:-$DATA_DIR/env}"
SERVICE_MODE="${POCKET_SERVICE:-ask}"
NON_INTERACTIVE=0
SKIP_BUILD=0
NO_RUSTUP=0
RESET_CONFIG=0
RESET_SERVICE=0
RESET_DATA=0
YES=0
DRY_RUN="${POCKET_DRY_RUN:-0}"
ORIGINAL_ARGS=("$@")

usage() {
  cat <<'USAGE'
Pocket Harness installer

Usage:
  ./install.sh [options]

Options:
  --standalone       Install binary/config only.
  --service          Install, enable, and start the Telegram service.
  --non-interactive  Use env vars/defaults and fail if required values are missing.
  --config PATH      Config path. Default: ~/.pocket-harness/config.yaml
  --data-dir PATH    Data dir. Default: ~/.pocket-harness
  --env-file PATH    Env file for secrets. Default: ~/.pocket-harness/env
  --skip-build       Use existing pocket-harness on PATH.
  --no-rustup        Do not install Rust automatically.
  --reset-config     Reset config/provider catalog/env after confirmation.
  --reset-service    Uninstall/reinstall service after confirmation.
  --reset-data       Reset runtime state/logs after confirmation.
  --yes              Confirm destructive reset prompts.
  --help             Show this help.

Environment:
  TELEGRAM_BOT_TOKEN, OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY, etc.
  POCKET_DRY_RUN=1 prints actions without changing the system.
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --standalone) SERVICE_MODE="standalone" ;;
    --service) SERVICE_MODE="service" ;;
    --non-interactive) NON_INTERACTIVE=1 ;;
    --config) CONFIG_PATH="$2"; shift ;;
    --data-dir) DATA_DIR="$2"; ENV_FILE="${POCKET_ENV_FILE:-$2/env}"; shift ;;
    --env-file) ENV_FILE="$2"; shift ;;
    --skip-build) SKIP_BUILD=1 ;;
    --no-rustup) NO_RUSTUP=1 ;;
    --reset-config) RESET_CONFIG=1 ;;
    --reset-service) RESET_SERVICE=1 ;;
    --reset-data) RESET_DATA=1 ;;
    --yes) YES=1 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown option: $1" >&2; usage; exit 2 ;;
  esac
  shift
done

LOG_DIR="$DATA_DIR/logs"
INSTALL_LOG="$LOG_DIR/install.log"

mkdir -p "$LOG_DIR"
touch "$INSTALL_LOG"
chmod 700 "$DATA_DIR" 2>/dev/null || true

log() {
  printf '%s\n' "$*" | tee -a "$INSTALL_LOG"
}

run() {
  log "+ $*"
  if [[ "$DRY_RUN" != "1" ]]; then
    "$@" 2>&1 | tee -a "$INSTALL_LOG"
  fi
}

need() {
  command -v "$1" >/dev/null 2>&1
}

is_source_checkout() {
  [[ -f Cargo.toml && -f providers.yaml && -f pocket-harness.yaml && -d src ]] || return 1
  grep -q '^name = "pocket-harness"' Cargo.toml
}

ensure_source_checkout() {
  is_source_checkout && return

  local repo="JamesFincher/pocket-harness"
  local repo_url="${POCKET_REPO_URL:-https://github.com/${repo}.git}"
  local checkout_dir="${POCKET_SOURCE_DIR:-}"

  if [[ -z "$checkout_dir" ]]; then
    checkout_dir="$(mktemp -d)/pocket-harness"
  fi

  log "Installer is not running from a Pocket Harness source checkout."
  log "Cloning source into: $checkout_dir"

  if [[ "$DRY_RUN" == "1" ]]; then
    log "Dry run complete. No source checkout was cloned."
    exit 0
  fi

  run git clone --depth 1 "$repo_url" "$checkout_dir"

  cd "$checkout_dir"
  exec bash ./install.sh "$@"
}

confirm() {
  local prompt="$1"
  if [[ "$YES" == "1" ]]; then
    return 0
  fi
  if [[ "$NON_INTERACTIVE" == "1" ]]; then
    log "non-interactive mode cannot confirm: $prompt"
    return 1
  fi
  read -r -p "$prompt [y/N] " answer
  [[ "$answer" == "y" || "$answer" == "Y" || "$answer" == "yes" || "$answer" == "YES" ]]
}

prompt() {
  local label="$1"
  local default="${2:-}"
  local secret="${3:-0}"
  local answer
  if [[ "$NON_INTERACTIVE" == "1" ]]; then
    printf '%s' "$default"
    return
  fi
  if [[ -n "$default" ]]; then
    label="$label [$default]"
  fi
  if [[ "$secret" == "1" ]]; then
    read -r -s -p "$label: " answer
    printf '\n' >&2
  else
    read -r -p "$label: " answer
  fi
  printf '%s' "${answer:-$default}"
}

install_packages() {
  local missing=()
  for cmd in git curl; do
    need "$cmd" || missing+=("$cmd")
  done
  [[ ${#missing[@]} -eq 0 ]] && return

  log "Missing prerequisites: ${missing[*]}"
  confirm "Install missing packages with sudo if needed?" || {
    log "Install cancelled. Please install: ${missing[*]}"
    exit 1
  }

  if need brew; then
    run brew install "${missing[@]}"
  elif need apt-get; then
    run sudo apt-get update
    run sudo apt-get install -y "${missing[@]}"
  elif need dnf; then
    run sudo dnf install -y "${missing[@]}"
  elif need yum; then
    run sudo yum install -y "${missing[@]}"
  elif need pacman; then
    run sudo pacman -Sy --needed --noconfirm "${missing[@]}"
  elif need zypper; then
    run sudo zypper install -y "${missing[@]}"
  elif need apk; then
    run sudo apk add "${missing[@]}"
  else
    log "No supported package manager found. Please install: ${missing[*]}"
    exit 1
  fi
}

install_rust() {
  if need cargo; then
    return
  fi
  if [[ "$NO_RUSTUP" == "1" ]]; then
    log "Cargo is missing and --no-rustup was supplied."
    exit 1
  fi
  log "Cargo not found; installing Rust with rustup."
  run sh -c 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y'
  # shellcheck disable=SC1091
  [[ -f "$HOME/.cargo/env" ]] && source "$HOME/.cargo/env"
}

binary() {
  if need pocket-harness; then
    command -v pocket-harness
  else
    printf '%s' "$HOME/.cargo/bin/pocket-harness"
  fi
}

write_env_value() {
  local key="$1"
  local value="$2"
  [[ -z "$value" ]] && return
  mkdir -p "$(dirname "$ENV_FILE")"
  touch "$ENV_FILE"
  chmod 600 "$ENV_FILE"
  if grep -q "^${key}=" "$ENV_FILE" 2>/dev/null; then
    local tmp
    tmp="$(mktemp)"
    awk -v key="$key" -v value="$value" 'BEGIN{done=0} $0 ~ "^" key "=" {print key "=" value; done=1; next} {print} END{if(!done) print key "=" value}' "$ENV_FILE" > "$tmp"
    mv "$tmp" "$ENV_FILE"
  else
    printf '%s=%s\n' "$key" "$value" >> "$ENV_FILE"
  fi
  chmod 600 "$ENV_FILE"
  export "$key=$value"
}

catalog_field() {
  local provider="$1"
  local field="$2"
  awk -v provider="$provider" -v field="$field" '
    $0 ~ "^  " provider ":" {in_provider=1; next}
    in_provider && $0 ~ /^  [A-Za-z0-9_-]+:/ {exit}
    in_provider && $1 == field ":" {
      sub("^[^:]+:[[:space:]]*", "")
      print
      exit
    }
  ' "$(dirname "$CONFIG_PATH")/providers.yaml"
}

choose_from_lines() {
  local title="$1"
  local lines="$2"
  local filter choice filtered
  while true; do
    filter="$(prompt "$title search (blank lists all)" "")"
    if [[ -n "$filter" ]]; then
      filtered="$(printf '%s\n' "$lines" | grep -i "$filter" || true)"
    else
      filtered="$lines"
    fi
    if [[ -z "$filtered" ]]; then
      log "No matches."
      continue
    fi
    printf '%s\n' "$filtered" | nl -w1 -s') '
    choice="$(prompt "Choose number or exact id" "1")"
    if [[ "$choice" =~ ^[0-9]+$ ]]; then
      printf '%s\n' "$filtered" | sed -n "${choice}p" | awk '{print $1}'
      return
    fi
    if printf '%s\n' "$filtered" | awk '{print $1}' | grep -Fxq "$choice"; then
      printf '%s' "$choice"
      return
    fi
    log "Invalid choice."
  done
}

ensure_config() {
  local bin="$1"
  if [[ "$RESET_CONFIG" == "1" && -e "$CONFIG_PATH" ]]; then
    confirm "Reset config, provider catalog, and env file?" || exit 1
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" reset config --yes
  fi

  if [[ ! -e "$CONFIG_PATH" ]]; then
    run "$bin" --config "$CONFIG_PATH" init --force
  else
    log "Preserving existing config: $CONFIG_PATH"
  fi
}

onboard() {
  local bin="$1"
  local telegram_token provider_lines provider model_lines model token_env provider_token base_url

  telegram_token="${TELEGRAM_BOT_TOKEN:-}"
  if [[ -z "$telegram_token" ]]; then
    telegram_token="$(prompt "Telegram bot token" "" 1)"
  fi
  if [[ -z "$telegram_token" ]]; then
    log "Telegram token is required for the Telegram gateway."
    exit 1
  fi
  write_env_value TELEGRAM_BOT_TOKEN "$telegram_token"
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set mobile.telegram.bot_token '$TELEGRAM_BOT_TOKEN'
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set mobile.telegram.enabled true

  local allowed_users
  allowed_users="$(prompt "Allowed Telegram user IDs as YAML list, blank allows private chats" "")"
  if [[ -n "$allowed_users" ]]; then
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set mobile.telegram.allowed_users "$allowed_users"
  fi

  provider_lines="$("$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" providers | awk '{print $1}')"
  provider="$(choose_from_lines "Provider" "$provider_lines")"
  base_url="$(catalog_field "$provider" base_url)"
  token_env="$(catalog_field "$provider" token_env)"
  model_lines="$("$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" models "$provider" | awk '/^- / {print substr($2,1)}')"
  model="$(choose_from_lines "Model" "$model_lines")"

  provider_token="${!token_env:-}"
  if [[ -z "$provider_token" ]]; then
    provider_token="$(prompt "$token_env for $provider (blank skips LLM router)" "" 1)"
  fi
  if [[ -n "$provider_token" ]]; then
    write_env_value "$token_env" "$provider_token"
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set llm_router.provider "$provider"
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set llm_router.base_url "$base_url"
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set llm_router.model "$model"
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set llm_router.api_key "\$$token_env"
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set features.llm_router.enabled true
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set llm_router.enabled true
  else
    log "Provider token skipped; LLM router remains disabled."
  fi

  local cwd
  cwd="$(prompt "Main thread cwd" "$HOME")"
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set threads.main.cwd "$cwd"
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set gateway.data_dir "$DATA_DIR"
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set gateway.hot_reload.enabled true
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set recovery.enabled true
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set recovery.on_connector_break rollback_to_last_known_good
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set recovery.write_rejection_report true
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" set threads.main.queue.enabled true
}

install_service() {
  local bin="$1"
  if [[ "$RESET_SERVICE" == "1" ]]; then
    confirm "Reset existing Pocket Harness service?" || exit 1
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" reset service --yes
  fi
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" service install
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" check --health
  run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" service status
}

main() {
  log "Pocket Harness installer"
  log "Config: $CONFIG_PATH"
  log "Env:    $ENV_FILE"
  log "Logs:   $LOG_DIR"

  install_packages
  ensure_source_checkout "$@"
  install_rust

  if [[ "$SKIP_BUILD" != "1" ]]; then
    run cargo install --path . --force
  fi

  local bin
  bin="$(binary)"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "Dry run complete. No config, env file, binary, or service changes were applied."
    exit 0
  fi

  ensure_config "$bin"

  if [[ "$RESET_DATA" == "1" ]]; then
    confirm "Reset runtime data and logs?" || exit 1
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" reset data --yes
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" reset logs --yes
  fi

  onboard "$bin"

  if [[ "$SERVICE_MODE" == "ask" && "$NON_INTERACTIVE" != "1" ]]; then
    if confirm "Install and start the Telegram service now?"; then
      SERVICE_MODE="service"
    else
      SERVICE_MODE="standalone"
    fi
  fi

  if [[ "$SERVICE_MODE" == "service" ]]; then
    install_service "$bin"
  else
    run "$bin" --config "$CONFIG_PATH" --env-file "$ENV_FILE" check --health
    log "Standalone install complete. Start Telegram with:"
    log "$bin --config \"$CONFIG_PATH\" --env-file \"$ENV_FILE\" telegram"
  fi
}

main "${ORIGINAL_ARGS[@]}"
