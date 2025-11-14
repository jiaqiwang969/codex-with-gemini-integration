# install-codex.ps1 — 在 Windows（PowerShell）安装预编译 Codex（非 npm）
# 从 GitHub Release 下载 codex-bundle-<triple>.tar.gz 并解压到 $HOME\.codex\bin
# 用法（安装最新发布）：
#   iwr https://raw.githubusercontent.com/<owner>/<repo>/main/scripts/install-codex.ps1 -UseBasicParsing | iex
# 指定版本：
#   $env:VERSION = "fork-v0.58.0-alpha.9-jw.1"; iwr https://raw.githubusercontent.com/<owner>/<repo>/main/scripts/install-codex.ps1 -UseBasicParsing | iex

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Info([string]$msg) {
  Write-Host "[install-codex] $msg"
}

$RepoOwner = $env:REPO_OWNER
if ([string]::IsNullOrWhiteSpace($RepoOwner)) { $RepoOwner = "jiaqiwang969" }
$RepoName = $env:REPO_NAME
if ([string]::IsNullOrWhiteSpace($RepoName)) { $RepoName = "127-BrickGPT-new" }
$Repo = "https://github.com/$RepoOwner/$RepoName"

function Detect-Triple {
  $os = $env:OS
  $arch = $env:PROCESSOR_ARCHITECTURE
  if ($arch -eq "ARM64") { return "aarch64-pc-windows-msvc" }
  elseif ($arch -eq "AMD64") { return "x86_64-pc-windows-msvc" }
  else { return "" }
}

function Resolve-Version {
  if ($env:VERSION) { return $env:VERSION }
  # 解析 /releases/latest 重定向获取 tag
  $resp = iwr "$Repo/releases/latest" -MaximumRedirection 0 -ErrorAction SilentlyContinue
  $loc = $resp.Headers["Location"]
  if (-not $loc) {
    # 一些环境可能直接返回 200，尝试读取最后一个重定向地址
    $final = (iwr "$Repo/releases/latest" -MaximumRedirection 10).BaseResponse.ResponseUri.AbsoluteUri
    return $final.Substring($final.LastIndexOf("/") + 1)
  }
  return $loc.Substring($loc.LastIndexOf("/") + 1)
}

function Main {
  $triple = Detect-Triple
  if (-not $triple) {
    Write-Info "不支持的平台/架构：$env:OS $env:PROCESSOR_ARCHITECTURE"
    exit 1
  }
  $ver = Resolve-Version
  $bundle = "codex-bundle-$triple.tar.gz"
  $assetUrl = "$Repo/releases/download/$ver/$bundle"

  Write-Info "目标平台：$triple"
  Write-Info "版本 tag：$ver"
  Write-Info "下载地址：$assetUrl"

  $tmp = New-Item -ItemType Directory -Path ([System.IO.Path]::GetTempPath()) -Name ("codex" + [System.Guid]::NewGuid()) -Force
  $bundlePath = Join-Path $tmp.FullName $bundle
  iwr $assetUrl -OutFile $bundlePath

  $dstBin = if ($env:PREFIX) { Join-Path $env:PREFIX "bin" } else { Join-Path $HOME ".codex\bin" }
  New-Item -ItemType Directory -Path $dstBin -Force | Out-Null

  Write-Info "解压到：$dstBin"
  # 需要 tar（Windows 10+ 自带）解包
  tar -xzf $bundlePath -C $dstBin

  Write-Info "安装完成。你可能需要把 $dstBin 添加到 PATH："
  Write-Host "  setx PATH `"$dstBin;%PATH%`""
  Write-Info "验证："
  Write-Host "  & `"$dstBin\codex.exe`" --version  或  codex --version"
}

Main

