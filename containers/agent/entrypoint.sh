#!/usr/bin/env bash
set -euo pipefail

truthy() {
    case "${1:-}" in
        1|true|TRUE|yes|YES|on|ON) return 0 ;;
        *) return 1 ;;
    esac
}

agent_home="${FINITECHAT_HOME:-/data/agent}"

restic_repository() {
    printf '%s' "${FINITE_AGENT_RESTIC_REPOSITORY:-${FINITE_DOCKER_RESTIC_REPOSITORY:-}}"
}

restic_password() {
    printf '%s' "${FINITE_AGENT_RESTIC_PASSWORD:-${FINITE_DOCKER_RESTIC_PASSWORD:-}}"
}

export_restic_env() {
    local password="$1"
    export RESTIC_PASSWORD="$password"
    export RESTIC_CACHE_DIR="${RESTIC_CACHE_DIR:-/tmp/restic-cache}"
}

restore_agent_state() {
    if ! truthy "${FINITE_AGENT_RESTORE_ON_START:-0}"; then
        return 0
    fi

    if [[ -f "$agent_home/config.json" ]] && ! truthy "${FINITE_AGENT_RESTORE_FORCE:-0}"; then
        echo "FINITE_AGENT_RESTORE_SKIPPED existing_state=true home=$agent_home"
        return 0
    fi

    local repository
    repository="$(restic_repository)"
    local password
    password="$(restic_password)"
    local snapshot="${FINITE_AGENT_RESTIC_SNAPSHOT_ID:-}"
    local tag="${FINITE_AGENT_RESTIC_BACKUP_TAG:-${FINITE_DOCKER_RESTIC_SNAPSHOT_TAG:-finite-agent-state}}"
    local target="${FINITE_AGENT_RESTIC_RESTORE_TARGET:-/}"

    if [[ -z "$repository" ]]; then
        echo "FINITE_AGENT_RESTORE_ERROR missing FINITE_AGENT_RESTIC_REPOSITORY" >&2
        return 64
    fi
    if [[ -z "$password" ]]; then
        echo "FINITE_AGENT_RESTORE_ERROR missing FINITE_AGENT_RESTIC_PASSWORD" >&2
        return 64
    fi
    if [[ -z "$snapshot" ]] && ! truthy "${FINITE_AGENT_RESTORE_LATEST:-0}"; then
        echo "FINITE_AGENT_RESTORE_ERROR missing FINITE_AGENT_RESTIC_SNAPSHOT_ID or FINITE_AGENT_RESTORE_LATEST=1" >&2
        return 64
    fi

    mkdir -p "$agent_home"
    export_restic_env "$password"
    if [[ -n "$snapshot" ]]; then
        echo "FINITE_AGENT_RESTORE_START snapshot=$snapshot home=$agent_home"
        restic -r "$repository" restore "$snapshot" --target "$target"
        echo "FINITE_AGENT_RESTORE_COMPLETE snapshot=$snapshot home=$agent_home"
    else
        echo "FINITE_AGENT_RESTORE_START snapshot=latest tag=$tag home=$agent_home"
        restic -r "$repository" restore latest --tag "$tag" --target "$target"
        echo "FINITE_AGENT_RESTORE_COMPLETE snapshot=latest tag=$tag home=$agent_home"
    fi
}

backup_agent_state() {
    if ! truthy "${FINITE_AGENT_BACKUP_ON_EXIT:-0}"; then
        return 0
    fi

    local repository
    repository="$(restic_repository)"
    local password
    password="$(restic_password)"
    local tag="${FINITE_AGENT_RESTIC_BACKUP_TAG:-${FINITE_DOCKER_RESTIC_SNAPSHOT_TAG:-finite-agent-state}}"

    if [[ -z "$repository" ]]; then
        echo "FINITE_AGENT_BACKUP_ERROR missing FINITE_AGENT_RESTIC_REPOSITORY" >&2
        return 64
    fi
    if [[ -z "$password" ]]; then
        echo "FINITE_AGENT_BACKUP_ERROR missing FINITE_AGENT_RESTIC_PASSWORD" >&2
        return 64
    fi
    if [[ ! -d "$agent_home" ]]; then
        echo "FINITE_AGENT_BACKUP_SKIPPED missing_home=true home=$agent_home"
        return 0
    fi

    export_restic_env "$password"
    echo "FINITE_AGENT_BACKUP_START home=$agent_home tag=$tag"
    restic -r "$repository" backup "$agent_home" --tag "$tag" --json
    echo "FINITE_AGENT_BACKUP_COMPLETE home=$agent_home tag=$tag"
}

restore_agent_state

if ! truthy "${FINITE_AGENT_SUPERVISE:-1}"; then
    exec "$@"
fi

"$@" &
child_pid="$!"
child_status=0
terminating=0

shutdown() {
    if [[ "$terminating" -eq 1 ]]; then
        return
    fi
    terminating=1
    if kill -0 "$child_pid" 2>/dev/null; then
        kill -TERM "$child_pid" 2>/dev/null || true
    fi
}

trap shutdown TERM INT

wait "$child_pid" || child_status="$?"
if [[ "$terminating" -eq 1 ]] && kill -0 "$child_pid" 2>/dev/null; then
    wait "$child_pid" || child_status="$?"
fi
backup_agent_state
exit "$child_status"
