# Builds the Android APK on Windows without needing Developer Mode.
#
# Why this script exists (both issues were seen on the dev PC, 2026-11-13):
#  1. `tauri android build` symlinks the Rust .so into jniLibs. Windows refuses
#     symlinks unless Developer Mode is on. We copy the file instead.
#  2. The JDK's internal pipe uses an AF_UNIX socket in %TEMP%. On this PC that
#     fails with "Unable to establish loopback connection". We point it at a
#     short plain folder instead.
#
# Usage:  powershell -ExecutionPolicy Bypass -File scripts\build-android.ps1 [-Target aarch64|armv7|x86_64] [-Debug]
param(
  [ValidateSet("aarch64", "armv7", "i686", "x86_64")] [string]$Target = "aarch64",
  [switch]$Debug
)
$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if (-not $env:ANDROID_HOME) { $env:ANDROID_HOME = "$env:LOCALAPPDATA\Android\Sdk" }
if (-not $env:JAVA_HOME) { $env:JAVA_HOME = "C:\Program Files\Android\Android Studio\jbr" }
if (-not $env:NDK_HOME) {
  $env:NDK_HOME = (Get-ChildItem "$env:ANDROID_HOME\ndk" | Sort-Object Name -Descending | Select-Object -First 1).FullName
}
$sock = "$env:USERPROFILE\.jsock"; New-Item -ItemType Directory -Force $sock | Out-Null
$env:JAVA_TOOL_OPTIONS = "-Djdk.net.unixdomain.tmpdir=$sock"

$map = @{
  aarch64 = @{ triple = "aarch64-linux-android"; abi = "arm64-v8a"; flavor = "Arm64" }
  armv7   = @{ triple = "armv7-linux-androideabi"; abi = "armeabi-v7a"; flavor = "Arm" }
  i686    = @{ triple = "i686-linux-android"; abi = "x86"; flavor = "X86" }
  x86_64  = @{ triple = "x86_64-linux-android"; abi = "x86_64"; flavor = "X86_64" }
}[$Target]
$profile = if ($Debug) { "debug" } else { "release" }
$Profile = if ($Debug) { "Debug" } else { "Release" }

Set-Location $root
pnpm build
if ($LASTEXITCODE) { throw "frontend build failed" }

# Same env the Tauri CLI sets so the NDK linker is used.
$tool = "$env:NDK_HOME\toolchains\llvm\prebuilt\windows-x86_64\bin"
$api = 24
$clang = if ($Target -eq "armv7") { "$tool\armv7a-linux-androideabi$api-clang.cmd" } else { "$tool\$($map.triple)$api-clang.cmd" }
$envName = "CARGO_TARGET_$($map.triple.ToUpper().Replace('-','_'))_LINKER"
Set-Item "env:$envName" $clang
Set-Item "env:CC_$($map.triple.Replace('-','_'))" $clang
Set-Item "env:AR_$($map.triple.Replace('-','_'))" "$tool\llvm-ar.exe"

$cargoArgs = @("build", "--manifest-path", "src-tauri\Cargo.toml", "--lib", "--target", $map.triple, "--features", "tauri/custom-protocol")
if (-not $Debug) { $cargoArgs += "--release" }
cargo @cargoArgs
if ($LASTEXITCODE) { throw "rust build failed" }

$jni = "src-tauri\gen\android\app\src\main\jniLibs\$($map.abi)"
New-Item -ItemType Directory -Force $jni | Out-Null
Copy-Item "src-tauri\target\$($map.triple)\$profile\libburger_os_lib.so" $jni -Force

Set-Location "src-tauri\gen\android"
.\gradlew.bat "assemble$($map.flavor)$Profile" -x "rustBuild$($map.flavor)$Profile" --console=plain
if ($LASTEXITCODE) { throw "gradle failed" }
$apk = Get-ChildItem -Recurse app\build\outputs\apk -Filter *.apk | Where-Object { $_.FullName -like "*$profile*" } |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $apk) { throw "no APK produced" }
Set-Location $root
$version = (Get-Content src-tauri\tauri.conf.json -Raw | ConvertFrom-Json).version
$signed = Test-Path "src-tauri\gen\android\keystore.properties"
$suffix = if ($Debug) { "-debug" } elseif ($signed) { "" } else { "-UNSIGNED" }
New-Item -ItemType Directory -Force release | Out-Null
$out = "release\burger-os-$version-$($map.abi)$suffix.apk"
Copy-Item $apk.FullName $out -Force
if (-not $Debug -and $signed) {
  $bt = Get-ChildItem "$env:ANDROID_HOME\build-tools" | Sort-Object Name -Descending | Select-Object -First 1
  & "$($bt.FullName)\apksigner.bat" verify --print-certs $out
  if ($LASTEXITCODE) { throw "signature check failed" }
  Write-Host "Signed release APK: $out"
} elseif (-not $Debug) {
  Write-Warning "No signing key (scripts\make-android-key.ps1 not run yet): $out is UNSIGNED and can't be installed as is."
} else {
  Write-Host "Debug APK: $out"
}
