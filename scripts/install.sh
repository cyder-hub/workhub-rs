#!/bin/sh
set -eu

REPO="cyder-hub/workhub-rs"
REPO_URL="https://github.com/${REPO}"
API_URL="https://api.github.com/repos/${REPO}/releases/latest"
BINARY_NAME="workhub"
INSTALL_DIR="${HOME}/.local/bin"
INSTALL_PATH="${INSTALL_DIR}/${BINARY_NAME}"

TMP_DIR=""
TMP_INSTALL=""

say() {
    printf '%s\n' "$*"
}

die() {
    say "Error: $*" >&2
    exit 1
}

cleanup() {
    if [ -n "${TMP_INSTALL}" ] && [ -f "${TMP_INSTALL}" ]; then
        rm -f "${TMP_INSTALL}"
    fi
    if [ -n "${TMP_DIR}" ] && [ -d "${TMP_DIR}" ]; then
        rm -rf "${TMP_DIR}"
    fi
}

trap cleanup EXIT INT TERM

has_command() {
    command -v "$1" >/dev/null 2>&1
}

require_tty() {
    if [ ! -r /dev/tty ] || [ ! -w /dev/tty ]; then
        die "an interactive terminal is required"
    fi
}

read_tty() {
    prompt="$1"
    printf '%s' "${prompt}" >/dev/tty
    IFS= read -r answer </dev/tty || answer=""
    printf '%s' "${answer}"
}

download_stdout() {
    url="$1"
    if has_command curl; then
        curl -fsSL -H "User-Agent: workhub-installer" "${url}"
    elif has_command wget; then
        wget -qO- --header="User-Agent: workhub-installer" "${url}"
    else
        die "curl or wget is required"
    fi
}

download_file() {
    url="$1"
    output="$2"
    if has_command curl; then
        curl -fsSL -H "User-Agent: workhub-installer" -o "${output}" "${url}"
    elif has_command wget; then
        wget -qO "${output}" --header="User-Agent: workhub-installer" "${url}"
    else
        die "curl or wget is required"
    fi
}

detect_asset() {
    os_raw="$(uname -s 2>/dev/null || true)"
    arch_raw="$(uname -m 2>/dev/null || true)"

    case "${os_raw}" in
        Linux) platform="linux" ;;
        Darwin) platform="darwin" ;;
        *) die "unsupported operating system: ${os_raw}" ;;
    esac

    case "${arch_raw}" in
        x86_64 | amd64) arch="x86_64" ;;
        arm64 | aarch64) arch="aarch64" ;;
        *) die "unsupported CPU architecture: ${arch_raw}" ;;
    esac

    ASSET="workhub-${platform}-${arch}"
    PLATFORM_LABEL="${platform}-${arch}"
}

latest_tag() {
    json="$(download_stdout "${API_URL}")" || die "failed to read latest GitHub release"
    tag="$(printf '%s\n' "${json}" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
    if [ -z "${tag}" ]; then
        die "latest GitHub release does not contain tag_name"
    fi
    printf '%s' "${tag}"
}

strip_v_prefix() {
    printf '%s' "$1" | sed 's/^v//'
}

installed_version() {
    if [ ! -e "${INSTALL_PATH}" ]; then
        printf '%s' "not installed"
        return
    fi

    if [ ! -x "${INSTALL_PATH}" ]; then
        printf '%s' "unknown"
        return
    fi

    version="$("${INSTALL_PATH}" -v 2>/dev/null | sed -n '1p' || true)"
    if [ -z "${version}" ]; then
        printf '%s' "unknown"
    else
        printf '%s' "${version}"
    fi
}

version_cmp() {
    a="$1"
    b="$2"

    case "${a}" in
        "" | *[!0-9.]*)
            printf '%s' "unknown"
            return
            ;;
    esac
    case "${b}" in
        "" | *[!0-9.]*)
            printf '%s' "unknown"
            return
            ;;
    esac

    old_ifs="${IFS}"
    IFS=.
    set -- ${a}
    a1="${1:-0}"
    a2="${2:-0}"
    a3="${3:-0}"
    IFS="${old_ifs}"

    IFS=.
    set -- ${b}
    b1="${1:-0}"
    b2="${2:-0}"
    b3="${3:-0}"
    IFS="${old_ifs}"

    if [ "${a1}" -lt "${b1}" ]; then
        printf '%s' "lt"
    elif [ "${a1}" -gt "${b1}" ]; then
        printf '%s' "gt"
    elif [ "${a2}" -lt "${b2}" ]; then
        printf '%s' "lt"
    elif [ "${a2}" -gt "${b2}" ]; then
        printf '%s' "gt"
    elif [ "${a3}" -lt "${b3}" ]; then
        printf '%s' "lt"
    elif [ "${a3}" -gt "${b3}" ]; then
        printf '%s' "gt"
    else
        printf '%s' "eq"
    fi
}

verify_checksum() {
    checksum_file="$1"

    if has_command sha256sum; then
        (cd "${TMP_DIR}" && sha256sum -c "${checksum_file##*/}") >/dev/null
    elif has_command shasum; then
        (cd "${TMP_DIR}" && shasum -a 256 -c "${checksum_file##*/}") >/dev/null
    else
        die "sha256sum or shasum is required"
    fi
}

path_contains_install_dir() {
    case ":${PATH:-}:" in
        *":${INSTALL_DIR}:"*) return 0 ;;
        *) return 1 ;;
    esac
}

profile_for_shell() {
    shell_name="$(basename "${SHELL:-}")"
    case "${shell_name}" in
        zsh) printf '%s' "${HOME}/.zshrc" ;;
        bash) printf '%s' "${HOME}/.bashrc" ;;
        *) printf '%s' "${HOME}/.profile" ;;
    esac
}

ensure_path_entry() {
    if path_contains_install_dir; then
        return
    fi

    profile="$(profile_for_shell)"
    if ! touch "${profile}" 2>/dev/null; then
        say "Install directory is not in PATH: ${INSTALL_DIR}"
        say "Add it to your shell profile to run workhub without the full path."
        return
    fi

    if grep -F "# workhub installer: begin" "${profile}" >/dev/null 2>&1; then
        say "Install directory is not active in PATH yet. Restart your shell or source ${profile}."
        return
    fi

    {
        printf '\n'
        printf '%s\n' '# workhub installer: begin'
        printf '%s\n' 'export PATH="$HOME/.local/bin:$PATH"'
        printf '%s\n' '# workhub installer: end'
    } >>"${profile}" || {
        say "Install directory is not in PATH: ${INSTALL_DIR}"
        say "Add it to your shell profile to run workhub without the full path."
        return
    }

    say "Added ${INSTALL_DIR} to PATH in ${profile}."
    say "Restart your shell or run: . ${profile}"
}

remove_path_entry() {
    for profile in "${HOME}/.zshrc" "${HOME}/.bashrc" "${HOME}/.profile"; do
        if [ ! -f "${profile}" ]; then
            continue
        fi
        if ! grep -F "# workhub installer: begin" "${profile}" >/dev/null 2>&1; then
            continue
        fi

        tmp_profile="${profile}.workhub-tmp.$$"
        if sed '/# workhub installer: begin/,/# workhub installer: end/d' "${profile}" >"${tmp_profile}" &&
            mv "${tmp_profile}" "${profile}"; then
            say "Removed workhub PATH entry from ${profile}."
        else
            rm -f "${tmp_profile}"
            say "Unable to remove workhub PATH entry from ${profile}."
        fi
    done
}

install_latest() {
    tag="$1"
    latest="$2"
    asset_url="${REPO_URL}/releases/download/${tag}/${ASSET}"
    checksum_url="${asset_url}.sha256"

    TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/workhub-install.XXXXXX")" || die "failed to create temporary directory"
    asset_file="${TMP_DIR}/${ASSET}"
    checksum_file="${TMP_DIR}/${ASSET}.sha256"

    say "Downloading ${ASSET}..."
    download_file "${asset_url}" "${asset_file}" || die "failed to download ${ASSET}"
    download_file "${checksum_url}" "${checksum_file}" || die "failed to download checksum"
    verify_checksum "${checksum_file}" || die "checksum verification failed"

    mkdir -p "${INSTALL_DIR}" || die "failed to create ${INSTALL_DIR}"
    TMP_INSTALL="${INSTALL_PATH}.tmp.$$"
    cp "${asset_file}" "${TMP_INSTALL}" || die "failed to stage binary"
    chmod 0755 "${TMP_INSTALL}" || die "failed to mark binary executable"
    mv "${TMP_INSTALL}" "${INSTALL_PATH}" || die "failed to install ${INSTALL_PATH}"
    TMP_INSTALL=""

    installed="$("${INSTALL_PATH}" -v 2>/dev/null | sed -n '1p' || true)"
    if [ "${installed}" != "${latest}" ]; then
        die "installed version check failed: expected ${latest}, got ${installed:-empty output}"
    fi

    say "Installed workhub ${installed} at ${INSTALL_PATH}."
    ensure_path_entry
}

uninstall_workhub() {
    if [ -e "${INSTALL_PATH}" ]; then
        rm -f "${INSTALL_PATH}" || die "failed to remove ${INSTALL_PATH}"
        say "Removed ${INSTALL_PATH}."
    else
        say "No workhub binary found at ${INSTALL_PATH}."
    fi

    remove_path_entry
    say "Uninstalled workhub."
}

prompt_install() {
    latest="$1"
    tag="$2"
    answer="$(read_tty "Install workhub ${latest}? [Y/n] ")"
    case "${answer}" in
        "" | y | Y | yes | YES | Yes) install_latest "${tag}" "${latest}" ;;
        *) say "Canceled." ;;
    esac
}

prompt_update_or_uninstall() {
    latest="$1"
    tag="$2"
    default_choice="$3"
    update_label="$4"

    say "Choose an action:"
    say "1. ${update_label}"
    say "2. Uninstall workhub"
    say "3. Cancel"
    answer="$(read_tty "Enter choice [${default_choice}]: ")"
    if [ -z "${answer}" ]; then
        answer="${default_choice}"
    fi

    case "${answer}" in
        1) install_latest "${tag}" "${latest}" ;;
        2) uninstall_workhub ;;
        3) say "Canceled." ;;
        *) say "Canceled." ;;
    esac
}

prompt_uninstall_or_cancel() {
    say "workhub is already up to date."
    say ""
    say "Choose an action:"
    say "1. Uninstall workhub"
    say "2. Cancel"
    answer="$(read_tty "Enter choice [2]: ")"
    if [ -z "${answer}" ]; then
        answer="2"
    fi

    case "${answer}" in
        1) uninstall_workhub ;;
        2) say "Canceled." ;;
        *) say "Canceled." ;;
    esac
}

main() {
    require_tty
    detect_asset

    tag="$(latest_tag)"
    latest="$(strip_v_prefix "${tag}")"
    current="$(installed_version)"

    say "workhub installer"
    say ""
    say "Platform: ${PLATFORM_LABEL}"
    say "Install path: ${INSTALL_PATH}"
    say "Installed: ${current}"
    say "Latest: ${latest}"
    say ""

    if [ "${current}" = "not installed" ]; then
        prompt_install "${latest}" "${tag}"
        return
    fi

    relation="$(version_cmp "${current}" "${latest}")"
    case "${relation}" in
        eq)
            prompt_uninstall_or_cancel
            ;;
        lt)
            prompt_update_or_uninstall "${latest}" "${tag}" "1" "Update to ${latest}"
            ;;
        gt)
            say "Installed version is newer than the latest GitHub release."
            say ""
            prompt_update_or_uninstall "${latest}" "${tag}" "3" "Reinstall ${latest}"
            ;;
        *)
            prompt_update_or_uninstall "${latest}" "${tag}" "1" "Install ${latest} over the current binary"
            ;;
    esac
}

main "$@"
