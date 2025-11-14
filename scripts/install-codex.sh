#!/usr/bin/env bash
#
# install-codex.sh — 安装预编译的 Codex 二进制（非 npm 方式）
# 使用 GitHub Release 中的跨平台打包资产 codex-bundle-<triple>.tar.gz
# 安装到 ~/.codex/bin（默认）或 /usr/local/bin（若有 sudo 权限可选）
#
# 用法（安装最新发布）：
#   bash -c "$(curl -fsSL https://raw.githubusercontent.com/<owner>/<repo>/main/scripts/install-codex.sh)"
# 或指定版本（对应 Release tag，例如 fork-v0.58.0-alpha.9-jw.1）：
#   VERSION=fork-v0.58.0-alpha.9-jw.1 bash -c "$(curl -fsSL https://raw.../install-codex.sh)"
#
set -euo pipefail

REPO_OWNER="${REPO_OWNER:-jiaqiwang969}"
REPO_NAME="${REPO_NAME:-127-BrickGPT-new}"
REPO="https://github.com/${REPO_OWNER}/${REPO_NAME}"

prefix_info() {
  printf "[install-codex] %s\n" "$*"
}

detect_triple() {
  local os arch triple
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux)
      case "$arch" in
        x86_64)  triple="x86_64-unknown-linux-musl" ;;
        aarch64|arm64) triple="aarch64-unknown-linux-musl" ;;
        *) triple="" ;;
      esac
      ;;
    Darwin)
      case "$arch" in
        x86_64) triple="x86_64-apple-darwin" ;;
        arm64)  triple="aarch64-apple-darwin" ;;
        *) triple="" ;;
      esac
      ;;
    MINGW*|MSYS*|CYGWIN*|Windows_NT)
      case "$arch" in
        x86_64) triple="x86_64-pc-windows-msvc" ;;
        arm64)  triple="aarch64-pc-windows-msvc" ;;
        *) triple="" ;;
      esac
      ;;
    *)
      triple=""
      ;;
  esac
  if [[ -z "$triple" ]]; then
    prefix_info "不支持的平台: ${os} ${arch}"
    exit 1
  fi
  echo "$triple"
}

resolve_version() {
  # VERSION 环境变量可指定目标 tag（如 fork-v0.58.0-alpha.9-jw.1）
  if [[ -n "${VERSION:-}" ]]; then
    echo "$VERSION"
    return
  fi
  # 解析最新 release 的 tag
  # 通过跟随重定向解析 /releases/latest 最终 URL
  local latest_url
  latest_url="$(curl -fsSLI -o /dev/null -w '%{url_effective}' "${REPO}/releases/latest")"
  # 末尾一般是 .../tag/<tag>
  echo "${latest_url##*/}"
}

ensure_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    prefix_info "缺少依赖命令：$1"
    exit 1
  }
}

main() {
  ensure_cmd curl
  ensure_cmd tar

  local triple ver asset_url tmp bundle_name
  triple="$(detect_triple)"
  ver="$(resolve_version)"
  bundle_name="codex-bundle-${triple}.tar.gz"
  asset_url="${REPO}/releases/download/${ver}/${bundle_name}"

  prefix_info "目标平台: ${triple}"
  prefix_info "版本 tag: ${ver}"
  prefix_info "下载地址: ${asset_url}"

  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  curl -fL -o "${tmp}/${bundle_name}" "${asset_url}"

  # 选择安装目录：默认 ~/.codex/bin；若用户传入 PREFIX 则用 PREFIX/bin
  local dst_bin
  if [[ -n "${PREFIX:-}" ]]; then
    dst_bin="${PREFIX%/}/bin"
  else
    dst_bin="${HOME}/.codex/bin"
  fi
  mkdir -p "${dst_bin}"

  prefix_info "解压安装到：${dst_bin}"
  tar -xzf "${tmp}/${bundle_name}" -C "${dst_bin}"
  chmod +x "${dst_bin}/codex" || true
  [[ -f "${dst_bin}/rg" ]] && chmod +x "${dst_bin}/rg" || true
  [[ -f "${dst_bin}/hunyuan-mcp-server" ]] && chmod +x "${dst_bin}/hunyuan-mcp-server" || true

  # 配置 PATH 提示
  if ! command -v codex >/dev/null 2>&1; then
    local shell_rc=""
    if [[ -n "${BASH_VERSION:-}" ]]; then shell_rc="${HOME}/.bashrc"; fi
    if [[ -n "${ZSH_VERSION:-}" ]]; then shell_rc="${HOME}/.zshrc"; fi
    if [[ -z "$shell_rc" ]]; then shell_rc="${HOME}/.profile"; fi
    if ! grep -qs "$dst_bin" "$shell_rc" 2>/dev/null; then
      prefix_info "将以下一行加入你的 shell rc（例如 ${shell_rc}）："
      echo "  export PATH=\"${dst_bin}:\$PATH\""
    fi
    prefix_info "本次会话可临时执行：export PATH=\"${dst_bin}:\$PATH\""
  fi

  prefix_info "安装完成。验证："
  echo "  ${dst_bin}/codex --version || codex --version"
}

main "$@"

